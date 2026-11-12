use {ch58x_hal as hal};
use ch58x_hal::{with_safe_access};
use hal::gpio::{Output};
use embedded_hal_local::delay::DelayNs;
use qingke::riscv::asm::{nop, wfi};

use core::ptr::{read_volatile};
use hal::delay::CycleDelay;
use ch58x_hal::pac::{ PFIC, SYS}; // for SYSTICK

use crate::log;


pub unsafe fn peek_register(ptr: u32){
    let dupa = read_volatile(ptr as *const u8); // read to ensure the write is complete
    log!("0x{:08X}: 0x{:02X}", ptr, dupa);
}

pub fn blinky(mut led: Output<'_, ch58x_hal::peripherals::PA8>) -> !{
    let mut delay = CycleDelay;
    loop{
        led.toggle();
        delay.delay_ms(100);
    }
}


pub fn enable_sleep(){
    let sys = unsafe { &*SYS::PTR };
     unsafe{
        with_safe_access(||{
            // wakeup from RTC ISR, memory stays active (?)
            sys.slp_wake_ctrl().modify(|_, w| {
                w.slp_rtc_wake().bit(true)
                    .slp_gpio_wake().bit(true)
                    .wake_ev_mode().bit(false)
            });
        });
        with_safe_access(||{
            // XXX cycles of delay
            sys.slp_power_ctrl().modify(|_, w| {
                w.wake_dly_mod().bits(0b00)
            });
        });

    }
}

pub fn enter_sleep(){
    let sys = unsafe { &*SYS::PTR };
    let pfic = unsafe { &*PFIC::PTR };

    unsafe{/*
        * R16 POWER_PLAN Register (Reset: 0x03FB)
        * 
        * [1] Bit 15:    RB_PWR_PLAN_EN   - Sleep planning enable
        [0010] Bit 14-11: RB_PWR_MUST_001_0 - Reserved (0010b)  
        * [0] Bit 10:    RB_PWR_DCDC_PRE  - DC-DC bias enable
        * [0] Bit 9:     RB_PWR_DCDC_EN   - DC-DC enable (immediate)
        * [1] Bit 8:     RB_PWR_LDO_EN    - Internal LDO control

        * [1] Bit 7:     RB_PWR_SYS_EN    - System power control
        * [1] Bit 6:     Reserved (write 0)
        * [0] Bit 5:     Reserved 
        * [1] Bit 4:     RB_PWR_RAM30K    - RAM30K power supply

        * [1] Bit 3:     RB_PWR_EXTEND    - USB/RF config power
        * [1] Bit 2:     RB_PWR_CORE      - Core/peripherals power
        * [1] Bit 1:     RB_PWR_RAM2K     - RAM2K power supply  
        * [1] Bit 0:     RB_PWR_XROM      - FlashROM power supply
        *
        * All bits control sleep planning except bits 10&9 (immediate effect)
        */

        //must use direct bits for now, pwr_plan_en is set as read only in SVD
        with_safe_access(||{
            sys.power_plan().modify(|_, w| {
                w.bits(0b1001_0001_0001_0111)
            });
        });

        // for even deeper sleep, todo
        // isp::flash_rom_reset();
        // sys.flash_cfg().write(|w| {
        //     w.bits(0x04)
        // });

        with_safe_access(|| {
            sys.xt32m_tune().write(|w| {
                w.xt32m_i_bias().bits(0b11)
            });
        });


        with_safe_access(|| {
            sys.pll_config().modify(|r, w| 
                w.bits(r.bits() | (1 << 5)));
        });

        pfic.sctlr().modify(|_, w| {
            w.sleepdeep().bit(true)
        });
        
        wfi();
        nop();
        nop();

        with_safe_access(|| {
            sys.pll_config().modify(|r, w| 
                w.bits(r.bits() & !(1 << 5)));
        });
    }
}


// the default safe access wrapper sometimes doesn't work on debug run
// #[inline(never)]
// #[no_mangle]
// pub unsafe extern "C" fn enable_rwa() -> bool {
//     let sys = unsafe { &*SYS::PTR };

//     let reg_ptr = 0x4000_1040 as *mut u8;

//     write_volatile(reg_ptr, 0x57);
//     nop();
//     write_volatile(reg_ptr, 0xA8);
// }