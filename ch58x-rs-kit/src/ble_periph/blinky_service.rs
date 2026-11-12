#![allow(static_mut_refs)]

use {ch58x_hal as hal};
use core::{ptr, slice};

use hal::ble::ffi::*;
use hal::ble::gap::*;
use hal::ble::gatt::*;
use hal::ble::gattservapp::*;
use hal::ble::{gatt_uuid, TmosEvent};
use hal::{ble};

use crate::ble_periph::handle_tmos_event;
use crate::log;

use embassy_executor::Spawner;
use embassy_futures::select::{select, Either};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Ticker, Timer};
use core::sync::atomic::{AtomicU32, Ordering};
use hal::gpio::{AnyPin, Level, Output, OutputDrive };

use super::{SYSTEM_ID, APP_CHANNEL, AppEvent};

pub const BLINKY_SERV_UUID: u16 = 0xFFE0;
pub const BLINKY_DATA_UUID: u16 = 0xFFE1;
pub const BLINKY_CONF_UUID: u16 = 0xFFE2;
pub const BLINKY_CMD_UUID: u16 = 0xFFE3;

pub static mut BLINKY_CLIENT_CHARCFG: [gattCharCfg_t; 4] = unsafe { core::mem::zeroed() };

static mut BLINKY_ATTR_TABLE: [GattAttribute; 6] =
    [
        // Blinky Service
        GattAttribute {
            type_: GattAttrType::PRIMARY_SERVICE,
            permissions: GATT_PERMIT_READ,
            handle: 0,
            value: &GattAttrType {
                len: ATT_BT_UUID_SIZE,
                uuid: &BLINKY_SERV_UUID as *const _ as _,
            } as *const _ as _,
        },
        // Blinky Data Declaration and Value
        GattAttribute {
            type_: GattAttrType::CHARACTERISTIC,
            permissions: GATT_PERMIT_READ,
            handle: 0,
            value: &(GATT_PROP_NOTIFY | GATT_PROP_READ) as *const _ as _,
        },
        GattAttribute {
            type_: GattAttrType::new_u16(&BLINKY_DATA_UUID),
            permissions: GATT_PERMIT_READ,
            handle: 0,
            value: ptr::null(), // this will be filled in read callback
        },
        // Blinky client config
        GattAttribute {
            type_: GattAttrType::CLIENT_CHAR_CFG,
            permissions: GATT_PERMIT_READ | GATT_PERMIT_WRITE,
            handle: 0,
            value: unsafe { BLINKY_CLIENT_CHARCFG.as_ptr() as _ },
        },
        // Command
        GattAttribute {
            type_: GattAttrType::CHARACTERISTIC,
            permissions: GATT_PERMIT_READ,
            handle: 0,
            value: &GATT_PROP_WRITE as *const _ as _,
        },
        GattAttribute {
            type_: GattAttrType::new_u16(&BLINKY_CMD_UUID),
            permissions: GATT_PERMIT_WRITE,
            handle: 0,
            value: ptr::null(),
        },
    ];


static BLINKY_ON: AtomicU32 = AtomicU32::new(1000);

#[embassy_executor::task]
pub async fn blinky_service_loop(pin: AnyPin) {
    let mut led = Output::new(pin, Level::Low, OutputDrive::_5mA);

    loop {
        let param = BLINKY_ON.load(Ordering::Relaxed);
        if param != 0 {
            led.set_high();
            Timer::after(Duration::from_millis((param/8).into())).await;
            led.set_low();
            Timer::after(Duration::from_millis((param - param/8).into())).await
        }        
    }
}


pub unsafe fn blinky_init() {
    unsafe extern "C" fn blinky_on_read_attr(
        _conn_handle: u16,
        attr: *mut GattAttribute,
        value: *mut u8,
        plen: *mut u16,
        offset: u16,
        _max_len: u16,
        _method: u8,
    ) -> u8 {
        // Make sure it's not a blob operation (no attributes in the profile are long)
        if offset > 0 {
            return ATT_ERR_ATTR_NOT_LONG;
        }

        let uuid = *((*attr).type_.uuid as *const u16);
        log!("! on_read_attr UUID: 0x{:04x}", uuid);

        match uuid {
            BLINKY_DATA_UUID => {
                let period = BLINKY_ON.load(Ordering::Relaxed);
                // let val: u8 = if on { 0x01 } else { 0x00 };
                *plen = size_of_val(&period) as _;
                core::ptr::copy(&period as *const _ as _, value, *plen as _);
            }
            _ => {
                return ATT_ERR_ATTR_NOT_FOUND;
            }
        }

        return 0;
    }
    unsafe extern "C" fn blinky_on_write_attr(
        conn_handle: u16,
        attr: *mut GattAttribute,
        value: *mut u8,
        len: u16,
        offset: u16,
        _method: u8,
    ) -> u8 {
        let uuid = *((*attr).type_.uuid as *const u16);
        log!("! on_write_attr UUID: 0x{:04x}", uuid);

        if uuid == BLINKY_CMD_UUID {
            let val = slice::from_raw_parts(value, len as usize);

            //convert val to u32
            if val.len() != 4 {
                return ATT_ERR_INVALID_VALUE_SIZE; // ATT_ERR_INVALID_VALUE_SIZE
            }
            let new_period = u32::from_le_bytes([val[0], val[1], val[2], val[3]]);
            log!("! on_write_attr cmd: 0x{:08x} len:{}", new_period ,len);

            if new_period < 10 || new_period > 5000 {
                return ATT_ERR_INVALID_VALUE
            }

            BLINKY_ON.store(new_period, Ordering::Relaxed);
            // if cmd == 0x01 {
            //     BLINKY_ON.store(true, Ordering::Relaxed);
            // } else if cmd == 0x00 {
            //     BLINKY_ON.store(false, Ordering::Relaxed);
            // }
        } else if uuid == BLINKY_CONF_UUID {
            // sub to notrification
            //  let status = GATTServApp::process_ccc_write_req(conn_handle, attr, value, len, offset, GATT_CLIENT_CFG_NOTIFY);
            // if status.is_ok() {
            //    log!("! on_write_attr sub");
            //    let val = slice::from_raw_parts(value, len as usize);
            //    log!("! on_write_attr sub value {:?}", val);
            // }
            //APP_CHANNEL.try_send(AppEvent::BlinkySubscribed(conn_handle));
            //log!("! on_write_attr sub");
        } else if uuid == gatt_uuid::GATT_CLIENT_CHAR_CFG_UUID {
            // client char cfg
            let status =
                GATTServApp::process_ccc_write_req(conn_handle, attr, value, len, offset, GATT_CLIENT_CFG_NOTIFY);
            if status.is_ok() {
                log!("! on_write_attr sub");
                let val = slice::from_raw_parts(value, len as usize);
                log!("! on_write_attr sub value {:?}", val);
                if val == &[0x01, 0x00] {
                    APP_CHANNEL.try_send(AppEvent::BlinkySubscribed(conn_handle)).ok();
                } else {
                    APP_CHANNEL.try_send(AppEvent::BlinkyUnsubscribed(conn_handle)).ok();
                }
            }
        } else {
            return ATT_ERR_ATTR_NOT_FOUND;
        }

        return 0;
    }

    static BLINKY_SERVICE_CB: gattServiceCBs_t = gattServiceCBs_t {
        pfnReadAttrCB: Some(blinky_on_read_attr),
        pfnWriteAttrCB: Some(blinky_on_write_attr),
        pfnAuthorizeAttrCB: None,
    };

    // Initialize Client Characteristic Configuration attributes
    GATTServApp::init_char_cfg(INVALID_CONNHANDLE, BLINKY_CLIENT_CHARCFG.as_mut_ptr());

    GATTServApp::register_service(
        &mut BLINKY_ATTR_TABLE[..],
        GATT_MAX_ENCRYPT_KEY_SIZE,
        &BLINKY_SERVICE_CB,
    )
    .unwrap();
}

#[embassy_executor::task]
pub async fn blinky_notification(conn_handle: u16) {
    let mut ticker = Ticker::every(Duration::from_millis(1000));

    static mut NOTIFY_MSG: gattMsg_t = gattMsg_t {
        handleValueNoti: attHandleValueNoti_t {
            handle: 0,
            len: 2,
            pValue: ptr::null_mut(),
        },
    };
    loop {
        match select(ticker.next(), APP_CHANNEL.receive()).await {
            Either::First(_) => unsafe {
                let val = GATTServApp::read_char_cfg(conn_handle, BLINKY_CLIENT_CHARCFG.as_ptr());
                if val == 0x01 {
                    // notification is no
                    let period = BLINKY_ON.load(Ordering::Relaxed);
                    // let val: u8 = if on { 0x01 } else { 0x00 };
                    // let mut msg = gattMsg_t::alloc_handle_value_notification(conn_handle, 2);
                    
                    NOTIFY_MSG.handleValueNoti.pValue =
                        GATT_bm_alloc(0, ATT_HANDLE_VALUE_NOTI, 2, ptr::null_mut(), 0) as _;
                    NOTIFY_MSG.handleValueNoti.handle = BLINKY_ATTR_TABLE[2].handle;
                    NOTIFY_MSG.handleValueNoti.len = 2;

                    core::ptr::copy(&period as *const _ as _, NOTIFY_MSG.handleValueNoti.pValue, 2);
                    log!("!! handle {}", BLINKY_ATTR_TABLE[2].handle);

                    let rc = GATT_Notification(conn_handle, &NOTIFY_MSG.handleValueNoti, 0);
                    log!("!! notify rc {:?}", rc);
                    
                }
            },
            Either::Second(AppEvent::Disconnected(_)) | Either::Second(AppEvent::BlinkyUnsubscribed(_)) => {
                log!("disconnect, stop notification");
                return;
            }
            _ => (),
        }
    }
}
