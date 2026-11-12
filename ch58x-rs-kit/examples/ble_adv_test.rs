//! Minimal cyberlamp advertiser. UART0/PB7 at 115200; LEDs stay off.
//! Check reception independently at 21:37:04:20:69:96 (name: CH582-test).

#![feature(impl_trait_in_assoc_type)]
#![no_std]
#![no_main]

use ch58x_hal as hal;
use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use hal::ble::ffi::*;
use hal::ble::gap::*;
use hal::ble;
use hal::gpio::{Level, Output, OutputDrive};
use hal::uart::UartTx;
use ch58x_rs_kit::logger::init as init_logger;
use ch58x_rs_kit::{log, tmos_mainloop};

static ADVERT_DATA: &[u8] = &[
    2,
    GAP_ADTYPE_FLAGS,
    GAP_ADTYPE_FLAGS_BREDR_NOT_SUPPORTED,
    11,
    GAP_ADTYPE_LOCAL_NAME_COMPLETE,
    b'C',
    b'H',
    b'5',
    b'8',
    b'2',
    b'-',
    b't',
    b'e',
    b's',
    b't',
];

unsafe extern "C" fn on_state_change(state: gapRole_States_t, event: *mut gapRoleEvent_t) {
    if event.is_null() {
        log!("GAP state={}", state);
    } else {
        log!(
            "GAP state={} opcode={} status={:?}",
            state,
            (*event).gap.opcode,
            (*event).gap.hdr.status
        );
    }
}

#[embassy_executor::main(entry = "qingke_rt::entry")]
async fn main(spawner: Spawner) -> ! {
    let mut config = hal::Config::default();
    config.clock.use_pll_60mhz();
    let p = hal::init(config);
    hal::embassy::init();
    let _oe = Output::new(p.PA8, Level::High, OutputDrive::_5mA);
    init_logger(UartTx::new(p.UART0, p.PB7, Default::default()).unwrap());
    log!("\n=== BLE advertising test ===");
    log!("BLE Lib Version: {}", ble::lib_version());

    let mut config = ble::Config::default();
    config.mac_addr = [0x21, 0x37, 0x04, 0x20, 0x69, 0x96].into();
    let (task_id, mut subscriber) = ble::init(config).unwrap();
    unsafe {
        GAPRole_PeripheralInit().unwrap();
        GAPRole_SetParameter(GAPROLE_ADVERT_DATA, ADVERT_DATA.len() as _, ADVERT_DATA.as_ptr() as _).unwrap();
        GAP_SetParamValue(TGAP_DISC_ADV_INT_MIN, 160).unwrap();
        GAP_SetParamValue(TGAP_DISC_ADV_INT_MAX, 160).unwrap();
        GAP_SetParamValue(TGAP_GEN_DISC_ADV_MIN, 0).unwrap();
        GAPRole_SetParameter(GAPROLE_ADVERT_ENABLED, 1, &1u8 as *const _ as _).unwrap();
        static BOND_CB: gapBondCBs_t = gapBondCBs_t {
            passcodeCB: None,
            pairStateCB: None,
            oobCB: None,
        };
        static APP_CB: gapRolesCBs_t = gapRolesCBs_t {
            pfnStateChange: Some(on_state_change),
            pfnRssiRead: None,
            pfnParamUpdate: None,
        };
        GAPRole_PeripheralStartDevice(task_id, &BOND_CB, &APP_CB).unwrap();
    }
    spawner.spawn(tmos_mainloop()).unwrap();
    loop {
        // Drain HAL messages so ownership/deallocation follows the normal peripheral path.
        match embassy_futures::select::select(subscriber.next_message_pure(), Timer::after(Duration::from_secs(5)))
            .await
        {
            embassy_futures::select::Either::First(event) => {
                log!("TMOS event={:#04x}", event.message_id());
            }
            embassy_futures::select::Either::Second(_) => {
                log!("Advertiser alive");
            }
        }
    }
}
