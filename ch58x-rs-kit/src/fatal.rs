//! Fatal-error shutdown: run the board's safe-state hook, report over UART0, halt.

use core::cell::Cell;
use core::fmt::{self, Write};

use critical_section::Mutex;

use ch58x_hal::peripherals;
use ch58x_hal::uart::UartTx;

static SAFE_STATE_HOOK: Mutex<Cell<Option<fn()>>> = Mutex::new(Cell::new(None));

/// Register a function that puts the board into a safe state (e.g. LEDs off) on a fatal error.
/// It runs with interrupts disabled and may steal peripherals: normal code never runs again.
pub fn set_safe_state_hook(hook: fn()) {
    critical_section::with(|cs| SAFE_STATE_HOOK.borrow(cs).set(Some(hook)));
}

/// Run the safe-state hook, report the fault, then halt without blinking or sleep.
/// Stealing peripherals is confined to this non-returning path: interrupts are
/// disabled and normal tasks will never use their original peripheral handles again.
pub fn halt(reason: fmt::Arguments<'_>) -> ! {
    let irq = qingke::register::gintenr::read();
    unsafe {
        qingke::register::gintenr::set_disable();
    }
    if let Some(hook) = critical_section::with(|cs| SAFE_STATE_HOOK.borrow(cs).get()) {
        hook();
    }
    let cause: u32;
    let pc: u32;
    let value: u32;
    let intsyscr: u32;
    unsafe {
        core::arch::asm!("csrr {}, mcause", out(reg) cause);
        core::arch::asm!("csrr {}, mepc", out(reg) pc);
        core::arch::asm!("csrr {}, mtval", out(reg) value);
        core::arch::asm!("csrr {}, 0x804", out(reg) intsyscr);
    }
    let interrupt_status = unsafe { core::ptr::read_volatile(0xe000_e04c as *const u32) };
    if let Ok(mut serial) = UartTx::new(
        unsafe { peripherals::UART0::steal() },
        unsafe { peripherals::PB7::steal() },
        Default::default(),
    ) {
        let _ = writeln!(serial, "\nFATAL: {}", reason);
        let _ = writeln!(
            serial,
            "CSR snapshot: mcause={:#x} mepc={:#x} mtval={:#x} irq={:#x}",
            cause, pc, value, irq
        );
        let _ = writeln!(serial, "halted");
        let _ = writeln!(serial, "Interrupt context: intsyscr={:#x} gisr={:#x}", intsyscr, interrupt_status);
    }
    loop {
        core::hint::spin_loop();
    }
}
