//! USB-flashable CH582 BLE keyboard PoC; active-low BOOT button on PB22.
//! USB is used by the ROM ISP bootloader, not as a USB keyboard/serial port.
#![feature(impl_trait_in_assoc_type)]
#![no_std]
#![no_main]

#[path = "ble_hid_keyboard/keyboard.rs"]
mod keyboard;
#[path = "ble_hid_keyboard/service.rs"]
mod service;

use ch58x_hal as hal;
use embassy_executor::Spawner;
use embassy_time::{Instant, Timer};
use core::sync::atomic::{AtomicU32, Ordering};
use hal::ble;
use hal::ble::ffi::*;
use hal::ble::gap::*;
use hal::ble::gattservapp::{GATTServApp, GATT_ALL_SERVICES};
use hal::gpio::{Input, Level, Output, OutputDrive, Pull};
use hal::uart::UartTx;
use ch58x_rs_kit::logger::init as init_logger;
use ch58x_rs_kit::log;

static TMOS_POLLS: AtomicU32 = AtomicU32::new(0);
static APP_POLLS: AtomicU32 = AtomicU32::new(0);

#[embassy_executor::task]
async fn tmos_mainloop() {
    let mut ticker = embassy_time::Ticker::every(embassy_time::Duration::from_micros(300));
    let mut polls: u32 = 0;
    loop {
        ticker.next().await;
        polls = polls.wrapping_add(1);
        TMOS_POLLS.store(polls, Ordering::Relaxed);
        unsafe {
            assert!(qingke::register::gintenr::read() & 8 != 0, "HID IRQ off before TMOS");
            TMOS_SystemProcess();
            assert!(qingke::register::gintenr::read() & 8 != 0, "HID IRQ off after TMOS");
        }
    }
}

const NAME: &[u8] = b"CH582-HID-PoC";
unsafe extern "C" {
    fn tmos_memory_get_free_min_len() -> u16;
    static gptrAESReg: *const u32;
    static gptrLLEReg: *const u32;
}
// Flags, complete HID/Battery service list, keyboard appearance 0x03c1.
const ADVERT: &[u8] = &[2, 1, 6, 5, 3, 0x12, 0x18, 0x0f, 0x18, 3, 0x19, 0xc1, 0x03];
const SCAN_RSP: &[u8] = &[
    14, 9, b'C', b'H', b'5', b'8', b'2', b'-', b'H', b'I', b'D', b'-', b'P', b'o', b'C', 5, 0x12, 12, 0, 24,
    0, // preferred connection interval:15–30ms
];

unsafe extern "C" fn state_changed(state: gapRole_States_t, event: *mut gapRoleEvent_t) {
    let role = state & GAPROLE_STATE_ADV_MASK;
    if event.is_null() {
        log!("HID GAP state={}", role);
    } else {
        log!("HID GAP state={} opcode={:#x} status={:?}", role, (*event).gap.opcode, (*event).gap.hdr.status);
    }
    if !event.is_null() && (*event).gap.opcode == GAP_LINK_ESTABLISHED_EVENT && role == GAPROLE_CONNECTED {
        let conn = (*event).linkCmpl.connectionHandle;
        service::connected(conn);
        log!("HID connected handle={}", conn);
    } else if !event.is_null() && (*event).gap.opcode == GAP_LINK_TERMINATED_EVENT {
        log!("HID disconnected reason={:#x}", (*event).linkTerminate.reason);
        service::reset();
        let enabled: u8 = 1;
        let _ = GAPRole_SetParameter(GAPROLE_ADVERT_ENABLED, 1, &enabled as *const _ as _);
    } else if role == GAPROLE_WAITING && service::connection().is_none() {
        let enabled: u8 = 1;
        let _ = GAPRole_SetParameter(GAPROLE_ADVERT_ENABLED, 1, &enabled as *const _ as _);
    }
}

unsafe extern "C" fn pairing(conn: u16, state: u8, status: u8) {
    log!("HID pairing: conn={} state={} status={:#x}", conn, state, status);
    log!("HID heap minimum free={}", tmos_memory_get_free_min_len());
}

unsafe extern "C" fn passcode(_address: *mut u8, conn: u16, inputs: u8, outputs: u8) {
    // WCH's HID reference answers the TK callback even for Just Works. With
    // NoInputNoOutput and MITM disabled, the temporary key/passcode is zero.
    log!("HID TK request: inputs={} outputs={}", inputs, outputs);
    let result = GAPBondMgr_PasscodeRsp(conn, 0, 0);
    log!("HID TK response: {:?}", result);
}

#[qingke_rt::entry]
fn main() -> ! {
    // Use the same continuously polled executor as cyberlamp, avoiding its
    // unresolved WFI/WFE wake issue. The local HAL includes the SysTick fix.
    let executor = embassy_executor::raw::Executor::new(core::ptr::null_mut());
    let executor: &'static embassy_executor::raw::Executor = unsafe { &*(&executor as *const _) };
    executor.spawner().spawn(app(executor.spawner())).unwrap();
    let mut last_log: u64 = 0;
    loop {
        unsafe {
            executor.poll();
        }
        assert!(qingke::register::gintenr::read() & 8 != 0, "HID: IRQ off after poll");
        let ticks = unsafe { (&*hal::pac::SYSTICK::PTR).cnt().read().bits() };
        if ticks.wrapping_sub(last_log) >= 22_500_000 {
            last_log = ticks;
            unsafe {
                log!("HID alive: tmos={} app={} AES={:#x} LLE={:#x}", TMOS_POLLS.load(Ordering::Relaxed), APP_POLLS.load(Ordering::Relaxed), gptrAESReg as usize, gptrLLEReg as usize);
            }
        }
    }
}

#[qingke_rt::interrupt]
fn Exception() {
    ch58x_rs_kit::fatal::halt(core::format_args!("HID CPU exception"))
}

#[embassy_executor::task]
async fn button_task(button: Input<'static, hal::peripherals::PB22>) {
    let mut debounce = keyboard::Button::new();
    loop {
        Timer::after_millis(5).await;
        if debounce.sample(button.is_low()) {
            service::tap_a().await;
        }
    }
}

#[embassy_executor::task]
async fn app(spawner: Spawner) -> ! {
    let mut config = hal::Config::default();
    config.clock.use_pll_60mhz();
    let p = hal::init(config);
    ch58x_rs_kit::fatal::set_safe_state_hook(leds_off);
    hal::embassy::init();
    // PA8 is the lamp's active-low LED driver enable. Keep the latched LED
    // outputs blanked when using the lamp board as a keyboard test device.
    let _led_output_disable = Output::new(p.PA8, Level::High, OutputDrive::_5mA);
    // Optional physical UART only; no probe or UART is needed for normal use.
    init_logger(UartTx::new(p.UART0, p.PB7, Default::default()).unwrap());
    let button = Input::new(p.PB22, Pull::Up);
    log!("\nBLE HID keyboard PoC: BOOT/PB22 sends A");
    // Retry a marginal measurement while the oscillator settles after reset;
    // retain the calibration routine's original accuracy requirement.
    let mut calibration = hal::ble::calibrate_lsi();
    for _ in 0..2 {
        if calibration.is_ok() { break; }
        Timer::after_millis(20).await;
        calibration = hal::ble::calibrate_lsi();
    }
    let hz = calibration.expect("BLE LSI calibration");
    log!("HID calibrated LSI: {} Hz", hz);
    let mut config = ble::Config::default();
    config.use_factory_mac_address(); // separate identity from the lamp
    log!("HID BLE address: {}", config.mac_addr);
    let (task_id, mut events) = ble::init(config).unwrap();
    unsafe {
        log!("HID heap after BLE init={}", tmos_memory_get_free_min_len());
        log!("HID register bases: AES={:#x} LLE={:#x}", gptrAESReg as usize, gptrLLEReg as usize);
    }
    unsafe {
        GAPRole_PeripheralInit().unwrap();
        GGS_AddService(GATT_ALL_SERVICES).unwrap();
        GATTServApp::add_service(GATT_ALL_SERVICES).unwrap();
        GGS_SetParameter(GGS_DEVICE_NAME_ATT, NAME.len() as _, NAME.as_ptr() as _).unwrap();
        let appearance: u16 = 0x03c1;
        GGS_SetParameter(GGS_APPEARANCE_ATT, 2, &appearance as *const _ as _).unwrap();
        GAPRole_SetParameter(GAPROLE_ADVERT_DATA, ADVERT.len() as _, ADVERT.as_ptr() as _).unwrap();
        GAPRole_SetParameter(GAPROLE_SCAN_RSP_DATA, SCAN_RSP.len() as _, SCAN_RSP.as_ptr() as _).unwrap();
        for param in [GAPROLE_PHY_TX_SUPPORTED, GAPROLE_PHY_RX_SUPPORTED] {
            let phy = GAP_PHY_BIT_LE_1M as u8;
            GAPRole_SetParameter(param, 1, &phy as *const _ as _).unwrap();
        }
        for (param, value) in [(GAPROLE_MIN_CONN_INTERVAL, 12u16), (GAPROLE_MAX_CONN_INTERVAL, 24)] {
            GAPRole_SetParameter(param, 2, &value as *const _ as _).unwrap();
        }
        // Just Works pairing: this board has no display or passkey keyboard.
        // Bonds survive reconnects, but the HAL currently keeps keys only in RAM.
        for (param, value) in [
            (GAPBOND_PERI_PAIRING_MODE, GAPBOND_PAIRING_MODE_WAIT_FOR_REQ),
            (GAPBOND_PERI_MITM_PROTECTION, 0),
            (GAPBOND_PERI_IO_CAPABILITIES, GAPBOND_IO_CAP_NO_INPUT_NO_OUTPUT),
            (GAPBOND_PERI_BONDING_ENABLED, 1),
            (GAPBOND_KEYSIZE, 16),
        ] {
            GAPBondMgr_SetParameter(param, 1, &value as *const _ as _).unwrap();
        }
        let mut security = [0u8; 5];
        for (index, param) in [GAPBOND_PERI_IO_CAPABILITIES, GAPBOND_PERI_MITM_PROTECTION,
            GAPBOND_PERI_BONDING_ENABLED, GAPBOND_PERI_SC_PROTECTION, GAPBOND_KEYSIZE].iter().enumerate() {
            if let Err(error) = GAPBondMgr_GetParameter(*param, &mut security[index] as *mut _ as _) {
                security[index] = 0xff;
                log!("HID security parameter {:#x} unavailable: {}", param, error);
            }
        }
        log!("HID security: io={} mitm={} bond={} sc={} key_size={}", security[0], security[1], security[2], security[3], security[4]);
        service::init();
        log!("HID heap after services={}", tmos_memory_get_free_min_len());
        GAP_SetParamValue(TGAP_DISC_ADV_INT_MIN, 160).unwrap();
        GAP_SetParamValue(TGAP_DISC_ADV_INT_MAX, 160).unwrap();
        GAP_SetParamValue(TGAP_GEN_DISC_ADV_MIN, 0).unwrap();
        GAP_SetParamValue(TGAP_LIM_ADV_TIMEOUT, 0).unwrap();
        let enabled: u8 = 1;
        GAPRole_SetParameter(GAPROLE_ADVERT_ENABLED, 1, &enabled as *const _ as _).unwrap();
        static BOND_CB: gapBondCBs_t = gapBondCBs_t {
            passcodeCB: Some(passcode),
            pairStateCB: Some(pairing),
            oobCB: None,
        };
        static APP_CB: gapRolesCBs_t = gapRolesCBs_t {
            pfnStateChange: Some(state_changed),
            pfnRssiRead: None,
            pfnParamUpdate: None,
        };
        GAPRole_PeripheralStartDevice(task_id, &BOND_CB, &APP_CB).unwrap();
    }
    spawner.spawn(tmos_mainloop()).unwrap();
    spawner.spawn(button_task(button)).unwrap();
    let mut connected_at = None;
    loop {
        APP_POLLS.store(APP_POLLS.load(Ordering::Relaxed).wrapping_add(1), Ordering::Relaxed);
        // Drain TMOS messages while polling the delayed connection update. No
        // await inside a GAP callback, and no automatic typing on connection.
        match embassy_futures::select::select(events.next_message_pure(), Timer::after_millis(50)).await {
            embassy_futures::select::Either::First(event) => {
                log!("HID TMOS message={:#x}", event.message_id());
                if event.message_id() == ble::TmosEvent::GATT_MSG_EVENT {
                    // TmosEvent drops the outer message only; ATT payload buffers
                    // have separate ownership, as in WCH's HID reference.
                    unsafe {
                        let msg = &mut *(event.0 as *mut ble::gatt::gattMsgEvent_t);
                        ble::gatt::GATT_bm_free(&mut msg.msg, msg.method);
                    }
                }
            }
            embassy_futures::select::Either::Second(_) => {}
        }
        match (service::connection(), connected_at) {
            (Some(conn), None) => connected_at = Some((conn, Instant::now(), false)),
            (Some(conn), Some((current, at, false))) if conn == current && at.elapsed().as_millis() >= 1000 => {
                unsafe {
                    let _ = GAPRole_PeripheralConnParamUpdateReq(conn, 12, 24, 0, 500, task_id);
                }
                connected_at = Some((conn, at, true));
            }
            (None, _) => connected_at = None,
            _ => {}
        }
    }
}

/// Fatal-error safe state: PA8 is the active-low LED output enable.
fn leds_off() {
    let oe = hal::gpio::Output::new(
        unsafe { hal::peripherals::PA8::steal() },
        hal::gpio::Level::High,
        hal::gpio::OutputDrive::_5mA,
    );
    core::mem::forget(oe);
}
