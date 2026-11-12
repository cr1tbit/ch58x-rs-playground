//! Litmus test for the QingKe V4A (CH58x) "A" extension and hardware register stacking (HPE).
//!
//! qingke 0.5 refuses to build for targets with native atomics unless the
//! `unsafe-trust-wch-atomics` feature is set ("most likely the atomics are broken").
//! This binary checks on real hardware what actually happens when thread-mode code using
//! LR/SC, AMO and DIV instructions is interrupted by an ISR that
//!   * clobbers every caller-saved register (so HPE must restore them), and
//!   * optionally touches the same memory word with its own AMO / LR-SC / plain store.
//!
//! The ISR is SysTick, routed through VTF channel 0 so it does not depend on the vector table.
//! Results go to UART0 (PB7, 115200) and to the `LITMUS_RES` array (read it with OpenOCD).
//!
//! Run: `cargo run --release --bin atomics_litmus`

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::ptr::{read_volatile, write_volatile};

use ch58x_hal as hal;
use hal::uart::UartTx;
use papajbadge_rs::log;
use papajbadge_rs::logger::init as init_logger;

// ISR behaviour selector, see `litmus_isr`.
const ISR_CLOBBER_ONLY: u32 = 0;
const ISR_AMO: u32 = 2;
const ISR_LRSC: u32 = 3;
const ISR_PLAIN_STORE: u32 = 4;
const ISR_DIV: u32 = 5;

#[no_mangle]
static mut LITMUS_MODE: u32 = 0;
#[no_mangle]
static mut LITMUS_ISR_COUNT: u32 = 0;
#[no_mangle]
static mut LITMUS_SHARED: u32 = 0;
#[no_mangle]
static mut LITMUS_RETRIES: u32 = 0;
/// Totals: [reg_errors, amo_value_errors, amo_lost, lrsc_value_errors, lrsc_lost_vs_lrsc,
///          lrsc_lost_vs_amo, lrsc_lost_vs_store, div_errors, isr_total, done_flag(0x600D600D)]
#[no_mangle]
static mut LITMUS_RES: [u32; 16] = [0; 16];
/// Written by `litmus_sentinel`: [events, register bitmask, last bad value, last register number]
#[no_mangle]
static mut LITMUS_CLOBBER: [u32; 4] = [0; 4];

extern "C" {
    fn litmus_isr();
    fn litmus_regs(n: u32) -> u32;
    fn litmus_amo(n: u32, ptr: *mut u32) -> u32;
    fn litmus_amo_rd_rs1(n: u32, ptr: *mut u32) -> u32;
    fn litmus_lrsc(n: u32, ptr: *mut u32) -> u32;
    fn litmus_lrsc_wide(n: u32, ptr: *mut u32) -> u32;
    fn litmus_div(n: u32) -> u32;
    fn litmus_sentinel() -> !;
}

global_asm!(
    r#"
    # The project builds for riscv32imc; the test needs the A instructions it is testing.
    .option push
    .option arch, +a
    .macro clobber_all
        li ra, 0xDEAD0001
        li t0, 0xDEAD0005
        li t1, 0xDEAD0006
        li t2, 0xDEAD0007
        li a0, 0xDEAD000A
        li a1, 0xDEAD000B
        li a2, 0xDEAD000C
        li a3, 0xDEAD000D
        li a4, 0xDEAD000E
        li a5, 0xDEAD000F
        li a6, 0xDEAD0010
        li a7, 0xDEAD0011
        li t3, 0xDEAD001C
        li t4, 0xDEAD001D
        li t5, 0xDEAD001E
        li t6, 0xDEAD001F
    .endm

    # ------------------------------------------------------------------ ISR
    # Like a qingke-rt 0.5 handler: no software register save, HPE only, ends in mret.
    .section .highcode.litmus, "ax"
    .align 2
    .global litmus_isr
litmus_isr:
    lui  t0, 0xE000F
    sw   zero, 4(t0)                # SysTick SR: clear CNTIF
    la   t0, LITMUS_ISR_COUNT
    lw   t1, 0(t0)
    addi t1, t1, 1
    sw   t1, 0(t0)
    la   t0, LITMUS_MODE
    lw   t1, 0(t0)
    la   t0, LITMUS_SHARED
    lui  t2, 0x10                   # ISR adds 0x10000, thread adds 1
    li   a0, 2
    beq  t1, a0, 20f
    li   a0, 3
    beq  t1, a0, 30f
    li   a0, 4
    beq  t1, a0, 40f
    li   a0, 5
    beq  t1, a0, 50f
    j    90f
20: amoadd.w zero, t2, (t0)
    j    90f
30: lr.w a1, (t0)
    add  a1, a1, t2
    sc.w a2, a1, (t0)
    bnez a2, 30b
    j    90f
40: lw   a1, 0(t0)
    add  a1, a1, t2
    sw   a1, 0(t0)
    j    90f
50: li   a1, 0x7fffffff
    li   a2, 3
    divu a3, a1, a2
    remu a4, a1, a2
90: clobber_all
    mret

    # ------------------------------------------------------------ thread side
    .section .text.litmus, "ax"

    .macro prologue
        addi sp, sp, -32
        sw ra, 28(sp)
        sw s0, 24(sp)
        sw s1, 20(sp)
        sw s2, 16(sp)
        sw s3, 12(sp)
        sw s4, 8(sp)
        sw s5, 4(sp)
    .endm
    .macro epilogue
        lw ra, 28(sp)
        lw s0, 24(sp)
        lw s1, 20(sp)
        lw s2, 16(sp)
        lw s3, 12(sp)
        lw s4, 8(sp)
        lw s5, 4(sp)
        addi sp, sp, 32
        ret
    .endm
    .macro chk reg, val
        li  s2, \val
        beq \reg, s2, 1f
        addi s1, s1, 1
    1:
    .endm
    # short data-dependent delay so the interrupt phase sweeps over the code under test
    .macro jitter
        andi s5, s3, 7
    1:  beqz s5, 2f
        addi s5, s5, -1
        j 1b
    2:
    .endm

    # u32 litmus_regs(n): every caller-saved register must survive interrupts
    .global litmus_regs
litmus_regs:
    prologue
    mv s0, a0
    li s1, 0
    li s3, 0
10: li ra, 0x01010101
    li t0, 0x05050505
    li t1, 0x06060606
    li t2, 0x07070707
    li a0, 0x0A0A0A0A
    li a1, 0x0B0B0B0B
    li a2, 0x0C0C0C0C
    li a3, 0x0D0D0D0D
    li a4, 0x0E0E0E0E
    li a5, 0x0F0F0F0F
    li a6, 0x10101010
    li a7, 0x11111111
    li t3, 0x1C1C1C1C
    li t4, 0x1D1D1D1D
    li t5, 0x1E1E1E1E
    li t6, 0x1F1F1F1F
    li s4, 24
11: addi s4, s4, -1
    bnez s4, 11b
    chk ra, 0x01010101
    chk t0, 0x05050505
    chk t1, 0x06060606
    chk t2, 0x07070707
    chk a0, 0x0A0A0A0A
    chk a1, 0x0B0B0B0B
    chk a2, 0x0C0C0C0C
    chk a3, 0x0D0D0D0D
    chk a4, 0x0E0E0E0E
    chk a5, 0x0F0F0F0F
    chk a6, 0x10101010
    chk a7, 0x11111111
    chk t3, 0x1C1C1C1C
    chk t4, 0x1D1D1D1D
    chk t5, 0x1E1E1E1E
    chk t6, 0x1F1F1F1F
    addi s3, s3, 1
    bne s3, s0, 10b
    mv a0, s1
    epilogue

    # u32 litmus_amo(n, ptr): thread does amoadd +1; old value's low half must equal i
    .global litmus_amo
litmus_amo:
    prologue
    mv s0, a0
    li s1, 0
    mv s4, a1
    li s3, 0
10: jitter
    li a5, 1
    mv a4, s4
    amoadd.w a3, a5, (a4)
    slli a3, a3, 16
    srli a3, a3, 16
    slli t0, s3, 16
    srli t0, t0, 16
    beq a3, t0, 12f
    addi s1, s1, 1
12: addi s3, s3, 1
    bne s3, s0, 10b
    mv a0, s1
    epilogue

    # same, with rd == rs1 like `amoor.w a1, a2, (a1)` in embassy's wake_task
    .global litmus_amo_rd_rs1
litmus_amo_rd_rs1:
    prologue
    mv s0, a0
    li s1, 0
    mv s4, a1
    li s3, 0
10: jitter
    li a5, 1
    mv a3, s4
    amoadd.w a3, a5, (a3)
    slli a3, a3, 16
    srli a3, a3, 16
    slli t0, s3, 16
    srli t0, t0, 16
    beq a3, t0, 12f
    addi s1, s1, 1
12: addi s3, s3, 1
    bne s3, s0, 10b
    mv a0, s1
    epilogue

    # u32 litmus_lrsc(n, ptr): compare-exchange style increment, as rustc emits it
    .global litmus_lrsc
litmus_lrsc:
    prologue
    mv s0, a0
    li s1, 0
    mv s4, a1
    li s3, 0
10: jitter
    mv a4, s4
11: lr.w a3, (a4)
    addi a5, a3, 1
    sc.w t1, a5, (a4)
    beqz t1, 12f
    la  t2, LITMUS_RETRIES
    lw  t3, 0(t2)
    addi t3, t3, 1
    sw  t3, 0(t2)
    j 11b
12: slli a3, a3, 16
    srli a3, a3, 16
    slli t0, s3, 16
    srli t0, t0, 16
    beq a3, t0, 13f
    addi s1, s1, 1
13: addi s3, s3, 1
    bne s3, s0, 10b
    mv a0, s1
    epilogue

    # same with a wider LR..SC window
    .global litmus_lrsc_wide
litmus_lrsc_wide:
    prologue
    mv s0, a0
    li s1, 0
    mv s4, a1
    li s3, 0
10: jitter
    mv a4, s4
11: lr.w a3, (a4)
    addi a5, a3, 1
    nop
    nop
    nop
    nop
    nop
    nop
    sc.w t1, a5, (a4)
    beqz t1, 12f
    la  t2, LITMUS_RETRIES
    lw  t3, 0(t2)
    addi t3, t3, 1
    sw  t3, 0(t2)
    j 11b
12: slli a3, a3, 16
    srli a3, a3, 16
    slli t0, s3, 16
    srli t0, t0, 16
    beq a3, t0, 13f
    addi s1, s1, 1
13: addi s3, s3, 1
    bne s3, s0, 10b
    mv a0, s1
    epilogue

    # u32 litmus_div(n): multi-cycle divu/remu interrupted by an ISR that also divides
    .global litmus_div
litmus_div:
    prologue
    mv s0, a0
    li s1, 0
    li s3, 0
10: jitter
    li a2, 0xFEDCBA98
    sub a2, a2, s3
    addi a3, s3, 1234
    divu a4, a2, a3
    remu a5, a2, a3
    mul t0, a4, a3
    add t0, t0, a5
    beq t0, a2, 12f
    addi s1, s1, 1
12: bltu a5, a3, 13f
    addi s1, s1, 1
13: addi s3, s3, 1
    bne s3, s0, 10b
    mv a0, s1
    epilogue

    # void litmus_sentinel(void) -> !: no interrupts, no memory traffic. Every register except
    # sp/gp/tp holds a known pattern; anything that changes one (i.e. a debug probe) is logged
    # in LITMUS_CLOBBER = [events, register bitmask, last bad value, last register number].
    .macro sentinel n, reg
        li  gp, 0x5A000000 + \n
        beq \reg, gp, 1f
        la  tp, LITMUS_CLOBBER
        lw  gp, 0(tp)
        addi gp, gp, 1
        sw  gp, 0(tp)
        sw  \reg, 8(tp)
        li  gp, \n
        sw  gp, 12(tp)
        li  \reg, 1 << \n
        lw  gp, 4(tp)
        or  gp, gp, \reg
        sw  gp, 4(tp)
        li  \reg, 0x5A000000 + \n
    1:
    .endm
    .global litmus_sentinel
litmus_sentinel:
    li ra, 0x5A000000 + 1
    li t0, 0x5A000000 + 5
    li t1, 0x5A000000 + 6
    li t2, 0x5A000000 + 7
    li s0, 0x5A000000 + 8
    li s1, 0x5A000000 + 9
    li a0, 0x5A000000 + 10
    li a1, 0x5A000000 + 11
    li a2, 0x5A000000 + 12
    li a3, 0x5A000000 + 13
    li a4, 0x5A000000 + 14
    li a5, 0x5A000000 + 15
    li a6, 0x5A000000 + 16
    li a7, 0x5A000000 + 17
    li s2, 0x5A000000 + 18
    li s3, 0x5A000000 + 19
    li s4, 0x5A000000 + 20
    li s5, 0x5A000000 + 21
    li s6, 0x5A000000 + 22
    li s7, 0x5A000000 + 23
    li s8, 0x5A000000 + 24
    li s9, 0x5A000000 + 25
    li s10, 0x5A000000 + 26
    li s11, 0x5A000000 + 27
    li t3, 0x5A000000 + 28
    li t4, 0x5A000000 + 29
    li t5, 0x5A000000 + 30
    li t6, 0x5A000000 + 31
10:
    sentinel 1, ra
    sentinel 5, t0
    sentinel 6, t1
    sentinel 7, t2
    sentinel 8, s0
    sentinel 9, s1
    sentinel 10, a0
    sentinel 11, a1
    sentinel 12, a2
    sentinel 13, a3
    sentinel 14, a4
    sentinel 15, a5
    sentinel 16, a6
    sentinel 17, a7
    sentinel 18, s2
    sentinel 19, s3
    sentinel 20, s4
    sentinel 21, s5
    sentinel 22, s6
    sentinel 23, s7
    sentinel 24, s8
    sentinel 25, s9
    sentinel 26, s10
    sentinel 27, s11
    sentinel 28, t3
    sentinel 29, t4
    sentinel 30, t5
    sentinel 31, t6
    j 10b
    .option pop
    "#
);

const PFIC_IENR0: *mut u32 = 0xE000_E100 as *mut u32;
const PFIC_IRER0: *mut u32 = 0xE000_E180 as *mut u32;
const PFIC_VTFIDR: *mut u32 = 0xE000_E050 as *mut u32;
const PFIC_VTFADDRR0: *mut u32 = 0xE000_E060 as *mut u32;
const STK_CTLR: *mut u32 = 0xE000_F000 as *mut u32;
const STK_SR: *mut u32 = 0xE000_F004 as *mut u32;
const STK_CMPLR: *mut u32 = 0xE000_F010 as *mut u32;
const STK_CMPHR: *mut u32 = 0xE000_F014 as *mut u32;
const SYSTICK_IRQ: u32 = 12;

/// Start a periodic SysTick interrupt every `period` HCLK cycles, handled by `litmus_isr`.
unsafe fn irq_start(mode: u32, period: u32) {
    LITMUS_MODE = mode;
    LITMUS_ISR_COUNT = 0;
    LITMUS_SHARED = 0;
    LITMUS_RETRIES = 0;
    write_volatile(STK_CTLR, 0);
    write_volatile(STK_SR, 0);
    write_volatile(STK_CMPLR, period);
    write_volatile(STK_CMPHR, 0);
    write_volatile(PFIC_VTFIDR, (read_volatile(PFIC_VTFIDR) & !0xff) | SYSTICK_IRQ);
    write_volatile(PFIC_VTFADDRR0, (litmus_isr as *const () as usize as u32) | 1);
    write_volatile(PFIC_IENR0, 1 << SYSTICK_IRQ);
    // STE | STIE | STCLK=HCLK | STRE (auto reload) | INIT
    write_volatile(STK_CTLR, 0x2f);
}

/// Stop the interrupt and return how many times the ISR ran.
unsafe fn irq_stop() -> u32 {
    write_volatile(PFIC_IRER0, 1 << SYSTICK_IRQ);
    write_volatile(STK_CTLR, 0);
    // let an already-latched interrupt drain before sampling the counters
    for _ in 0..64 {
        core::arch::asm!("nop");
    }
    read_volatile(&raw const LITMUS_ISR_COUNT)
}

const PERIODS: [u32; 4] = [1499, 997, 601, 353];
const N: u32 = 50_000; // < 65536: thread and ISR share one word, 16 bits each
const ROUNDS: u32 = 8;

struct Totals {
    value_errors: u32,
    lost: u32,
    isr: u32,
    retries: u32,
}

/// Run `f` on the shared word against ISR `mode`; returns per-iteration value errors and
/// lost updates (thread half or ISR half of the word not matching what was added).
unsafe fn run_shared(mode: u32, f: unsafe extern "C" fn(u32, *mut u32) -> u32) -> Totals {
    let mut t = Totals { value_errors: 0, lost: 0, isr: 0, retries: 0 };
    for round in 0..ROUNDS {
        irq_start(mode, PERIODS[(round % 4) as usize]);
        let errs = f(N, &raw mut LITMUS_SHARED);
        let k = irq_stop();
        let shared = read_volatile(&raw const LITMUS_SHARED);
        let isr_touches = mode == ISR_AMO || mode == ISR_LRSC || mode == ISR_PLAIN_STORE;
        let expect_hi = if isr_touches { k & 0xffff } else { 0 };
        t.value_errors += errs;
        t.lost += (N & 0xffff).abs_diff(shared & 0xffff) + expect_hi.abs_diff(shared >> 16);
        t.isr += k;
        t.retries += read_volatile(&raw const LITMUS_RETRIES);
    }
    t
}

unsafe fn report(name: &str, t: &Totals) {
    log!(
        "{:<34} value_errors={:<6} lost_updates={:<6} sc_retries={:<6} isr_runs={}",
        name,
        t.value_errors,
        t.lost,
        t.retries,
        t.isr
    );
}

#[qingke_rt::entry]
fn main() -> ! {
    let mut config = hal::Config::default();
    config.clock.use_pll_60mhz();
    let p = hal::init(config);
    let serial = UartTx::new(p.UART0, p.PB7, Default::default()).unwrap();
    init_logger(serial);

    let intsyscr: usize;
    unsafe { core::arch::asm!("csrr {}, 0x804", out(reg) intsyscr) };
    log!("\n=== QingKe V4A atomics / HPE litmus === INTSYSCR={:#x}", intsyscr);

    unsafe {
        let mut res = [0u32; 16];

        // 1. HPE: registers must survive an ISR that clobbers all of them
        let mut errs = 0;
        let mut isr = 0;
        for round in 0..ROUNDS {
            irq_start(ISR_CLOBBER_ONLY, PERIODS[(round % 4) as usize]);
            errs += litmus_regs(N);
            isr += irq_stop();
        }
        log!("{:<34} reg_errors={:<8} isr_runs={}", "HPE register restore", errs, isr);
        res[0] = errs;
        res[8] += isr;

        // 2. AMO in thread
        let t = run_shared(ISR_CLOBBER_ONLY, litmus_amo);
        report("AMO, idle ISR", &t);
        res[1] += t.value_errors;
        res[2] += t.lost;
        let t = run_shared(ISR_CLOBBER_ONLY, litmus_amo_rd_rs1);
        report("AMO rd==rs1, idle ISR", &t);
        res[1] += t.value_errors;
        res[2] += t.lost;
        let t = run_shared(ISR_AMO, litmus_amo);
        report("AMO vs AMO in ISR", &t);
        res[1] += t.value_errors;
        res[2] += t.lost;
        let t = run_shared(ISR_AMO, litmus_amo_rd_rs1);
        report("AMO rd==rs1 vs AMO in ISR", &t);
        res[1] += t.value_errors;
        res[2] += t.lost;
        let t = run_shared(ISR_LRSC, litmus_amo);
        report("AMO vs LR/SC in ISR", &t);
        res[1] += t.value_errors;
        res[2] += t.lost;

        // 3. LR/SC in thread
        let t = run_shared(ISR_CLOBBER_ONLY, litmus_lrsc);
        report("LR/SC, idle ISR", &t);
        res[3] += t.value_errors + t.lost;
        let t = run_shared(ISR_LRSC, litmus_lrsc);
        report("LR/SC vs LR/SC in ISR (must be 0)", &t);
        res[3] += t.value_errors;
        res[4] += t.lost;
        let t = run_shared(ISR_LRSC, litmus_lrsc_wide);
        report("LR/SC wide vs LR/SC in ISR", &t);
        res[3] += t.value_errors;
        res[4] += t.lost;
        let t = run_shared(ISR_AMO, litmus_lrsc);
        report("LR/SC vs AMO in ISR", &t);
        res[3] += t.value_errors;
        res[5] += t.lost;
        let t = run_shared(ISR_AMO, litmus_lrsc_wide);
        report("LR/SC wide vs AMO in ISR", &t);
        res[3] += t.value_errors;
        res[5] += t.lost;
        let t = run_shared(ISR_PLAIN_STORE, litmus_lrsc_wide);
        report("LR/SC wide vs plain store in ISR", &t);
        res[3] += t.value_errors;
        res[6] += t.lost;

        // 4. DIV
        let mut errs = 0;
        let mut isr = 0;
        for round in 0..ROUNDS {
            irq_start(ISR_DIV, PERIODS[(round % 4) as usize]);
            errs += litmus_div(N);
            isr += irq_stop();
        }
        log!("{:<34} div_errors={:<8} isr_runs={}", "DIV vs DIV in ISR", errs, isr);
        res[7] = errs;
        res[8] += isr;

        res[9] = 0x600D_600D;
        LITMUS_RES = res;
        log!("=== done ===");
    }

    // 5. Sit in a register-sentinel loop: attach a debugger and watch LITMUS_CLOBBER.
    unsafe { litmus_sentinel() }
}
