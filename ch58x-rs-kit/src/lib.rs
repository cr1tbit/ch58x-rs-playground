#![feature(impl_trait_in_assoc_type)]
#![feature(type_alias_impl_trait)]
#![feature(generic_const_exprs)]
#![no_std]


use {ch58x_hal as hal};
use ch58x_hal::peripherals;
use hal::uart::UartTx;
use hal::rtc::{DateTime, Rtc};


pub mod helpers;
pub mod logger;
pub mod settings_store;
pub mod sr_led_driver;

pub mod ble_periph;
pub mod fatal;

#[allow(unused)]
static mut SERIAL: Option<UartTx<peripherals::UART0>> = None;


/// No longer needed: `hal::embassy::Executor` runs the BLE stack (TMOS) whenever it has work
/// (see `ch58x_hal::ble::stack`). Polling it every 300 µs kept the CPU awake ~3300 times a
/// second. Kept so existing firmware still builds; spawning it does nothing.
#[embassy_executor::task]
pub async fn tmos_mainloop() {}

pub fn get_configured_rtc() -> Rtc {
    let mut rtc = Rtc {};
    let now = rtc.now();

    if now.year < 2025 {
        log!("RTC not set, setting to a meaningless random date");
        rtc.set_datatime(
            DateTime { year: 2021, month: 4, day: 2,
                hour: 21,minute: 36, second: 30,
                millisecond: 0,
            }
        );
    } else {
        log!("RTC already set to: {:02}:{:02}:{:02} {:02}/{:02}/{}", 
            now.hour, now.minute, now.second, now.month, now.day, now.year);
    }
    rtc
}



#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    fatal::halt(core::format_args!("{}", info))
}
