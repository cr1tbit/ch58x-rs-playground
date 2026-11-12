#![feature(impl_trait_in_assoc_type)]
#![feature(type_alias_impl_trait)]
#![no_std]
#![no_main]

use ch58x_hal::ble::MacAddress;
use ch58x_hal as hal;
use embassy_executor::Spawner;
use hal::gpio::{Level, Output, OutputDrive, Pin};
use hal::uart::UartTx;
use hal::{ble};
use ch58x_rs_kit::logger::init as init_logger;
use ch58x_rs_kit::{get_configured_rtc, log, tmos_mainloop};

use ch58x_rs_kit::ble_periph::{common_init, devinfo_init, peripheral};
// use ch58x_rs_kit::ble_periph::blinky_service::{blinky_init, blinky_service_loop};
use ch58x_rs_kit::ble_periph::current_time_service::current_time_init;
// use ch58x_rs_kit::ble_periph::hid_service::hid_init; // added

#[qingke_rt::entry]
fn main() -> ! {
    hal::embassy::Executor::new().run(|spawner| spawner.must_spawn(app_main(spawner)))
}

#[embassy_executor::task]
async fn app_main(spawner: Spawner) -> ! {
    let mut config = hal::Config::default();
    config.clock.use_pll_60mhz().enable_lse();
    let p = hal::init(config);
    hal::embassy::init();

    let mut ena = Output::new(p.PA4, Level::Low, OutputDrive::_5mA);
    ena.set_low();

    let serial = UartTx::new(p.UART0, p.PB7, Default::default()).unwrap();
    init_logger(serial);
    log!("\n\n\nHello World!");

    // spawner.spawn(blinky_service_loop(p.PA8.degrade())).unwrap();

    let rtc = get_configured_rtc();

    log!("System Clocks: {}", hal::sysctl::clocks().hclk);
    log!("ChipID: 0x{:02x}", hal::signature::get_chip_id());
    log!("RTC datetime: {}", rtc.now());

    log!("BLE Lib Version: {}", ble::lib_version());

    let mut ble_config = ble::Config::default();
    ble_config.pa_config = None;
    ble_config.mac_addr = build_mac();
    log!("BLE MAC: {:02x?}", ble_config.mac_addr);
    let (task_id, sub) = hal::ble::init(ble_config).unwrap();
    log!("BLE hal task id: {}", task_id);

    unsafe {
        common_init();
        devinfo_init();
        // blinky_init();
        current_time_init();
        // hid_init(); // added
    }

    // Main_Circulation
    spawner.spawn(tmos_mainloop()).unwrap();

    // Application code
    peripheral(spawner, task_id, sub, rtc, None).await
}

fn build_mac() -> MacAddress {
    let uid = hal::isp::get_unique_id();

    let mut mac = [0u8; 6];
    mac[0] = 0x21;
    mac[1] = 0x37;

    for i in 0..4 {
        mac[i + 2] = uid[i];
    }

    mac.into()
}
