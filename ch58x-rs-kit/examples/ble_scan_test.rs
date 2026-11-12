//! BLE receive test for the cyberlamp board: scan for advertisers and print what is heard.
//!
//! Receiving reports verifies the receive path and BLE scan scheduling. This passive
//! test does not exercise transmission; use `ble_adv_test` and an independent scanner
//! to verify advertising.
//!
//! Adapted from ch58x-hal `examples/ble-scanner.rs`. UART0 on PB7, 115200.

#![feature(impl_trait_in_assoc_type)]
#![feature(type_alias_impl_trait)]
#![no_std]
#![no_main]

use ch58x_hal as hal;
use embassy_executor::Spawner;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Timer};
use hal::ble;
use hal::ble::ffi::*;
use hal::ble::gap::*;
use hal::ble::MacAddress;
use hal::gpio::{Level, Output, OutputDrive};
use hal::uart::UartTx;
use ch58x_rs_kit::logger::init as init_logger;
use ch58x_rs_kit::{log, tmos_mainloop};

type CS = embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

static EVENTS: Channel<CS, bool, 10> = Channel::new();
static mut HEARD: u32 = 0;

const DISCOVERY_MODE: u8 = DEVDISC_MODE_ALL;
const ACTIVE_SCAN: u8 = 0; // passive: receive only, never transmit
const WHITE_LIST: u8 = 0;

unsafe extern "C" fn observer_event_callback(event: &gapRoleEvent_t) {
    match event.gap.opcode {
        GAP_DEVICE_INIT_DONE_EVENT => {
            log!("Scanning...");
            GAPRole_ObserverStartDiscovery(DISCOVERY_MODE, ACTIVE_SCAN, WHITE_LIST).unwrap();
        }
        GAP_DEVICE_DISCOVERY_EVENT => {
            log!("Scan window complete, {} reports so far", HEARD);
            let _ = EVENTS.try_send(true);
        }
        GAP_DEVICE_INFO_EVENT => {
            let info = event.deviceInfo;
            HEARD += 1;
            log!("Device => {}, rssi={}, len={}", MacAddress::from_raw(info.addr), info.rssi, info.dataLen);
        }
        other => {
            log!("unknown event opcode: {}", other);
        }
    }
}

#[embassy_executor::task]
async fn observer() {
    static CALLBACK: gapRoleObserverCB_t = gapRoleObserverCB_t {
        eventCB: Some(observer_event_callback),
    };
    unsafe {
        GAPRole_ObserverStartDevice(&CALLBACK).unwrap();
    }

    loop {
        EVENTS.receiver().receive().await;
        unsafe {
            GAPRole_ObserverStartDiscovery(DISCOVERY_MODE, ACTIVE_SCAN, WHITE_LIST).unwrap();
        }
    }
}

#[qingke_rt::entry]
fn main() -> ! {
    hal::embassy::Executor::new().run(|spawner| spawner.must_spawn(app_main(spawner)))
}

#[embassy_executor::task]
async fn app_main(spawner: Spawner) -> ! {
    let mut config = hal::Config::default();
    config.clock.use_pll_60mhz();
    let p = hal::init(config);
    hal::embassy::init();

    // /OE of the LED drivers: keep the LEDs off
    let _oe = Output::new(p.PA8, Level::High, OutputDrive::_5mA);

    let serial = UartTx::new(p.UART0, p.PB7, Default::default()).unwrap();
    init_logger(serial);
    log!("\n\n=== BLE scan test ===");
    log!("BLE Lib Version: {}", ble::lib_version());

    let (task_id, _sub) = ble::init(ble::Config::default()).unwrap();
    log!("BLE init task id: {}", task_id);

    unsafe {
        GAPRole_ObserverInit().unwrap();
        // 4800 * 0.625 ms = 3 s per scan window
        GAP_SetParamValue(TGAP_DISC_SCAN, 4800).unwrap();
        GAP_SetParamValue(TGAP_DISC_SCAN_PHY, GAP_PHY_BIT_LE_1M).unwrap();
    }

    spawner.spawn(observer()).unwrap();
    spawner.spawn(tmos_mainloop()).unwrap();

    loop {
        Timer::after(Duration::from_secs(60)).await;
    }
}
