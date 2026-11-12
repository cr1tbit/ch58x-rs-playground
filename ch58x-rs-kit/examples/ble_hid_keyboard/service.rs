//! Standalone HOGP keyboard PoC. GAP/GATT callbacks and this API run only on
//! the single executor/TMOS thread, never from an interrupt handler.
#![allow(static_mut_refs)]

use core::{ptr, slice};

use ch58x_hal::ble::ffi::*;
use ch58x_hal::ble::gap::*;
use ch58x_hal::ble::gatt::*;
use ch58x_hal::ble::gatt_uuid::*;
use ch58x_hal::ble::gattservapp::*;
use ch58x_hal::ble::linkdb::*;
use embassy_time::Timer;
use ch58x_rs_kit::log;

use super::keyboard::{self, KEY_A, RELEASE, REPORT_ID, REPORT_MAP};

const INFO: [u8; 4] = [0x11, 0x01, 0, 0x02]; // HID 1.11; normally connectable, no remote wake
const INPUT_REF: [u8; 2] = [REPORT_ID, 1];
const OUTPUT_REF: [u8; 2] = [REPORT_ID, 2];
// Development identity, not an assigned commercial product identifier.
const PNP: [u8; 7] = [1, 0xff, 0xff, 0x82, 0x05, 0x00, 0x01];
const BATTERY: [u8; 1] = [100]; // USB-powered PoC, no battery measurement

#[derive(Clone, Copy, PartialEq, Eq)]
struct Session {
    conn: u16,
    epoch: u32,
}

#[derive(Clone, Copy)]
struct State {
    session: Session,
    protocol: u8,
    suspended: bool,
    report: [u8; 8],
    leds: u8,
}
static mut STATE: State = State {
    session: Session {
        conn: INVALID_CONNHANDLE,
        epoch: 0,
    },
    protocol: 1,
    suspended: false,
    report: RELEASE,
    leds: 0,
};
static mut REPORT_CCCD: [gattCharCfg_t; 4] = unsafe { core::mem::zeroed() };
static mut BOOT_CCCD: [gattCharCfg_t; 4] = unsafe { core::mem::zeroed() };

const fn attr(uuid: &'static u16, permissions: u8, value: *const u8) -> GattAttribute {
    GattAttribute {
        type_: GattAttrType::new_u16(uuid),
        permissions,
        handle: 0,
        value,
    }
}
const fn declaration(properties: &'static u8) -> GattAttribute {
    attr(&GATT_CHARACTER_UUID, GATT_PERMIT_READ, properties as *const u8)
}
const fn primary(service: &'static GattAttrType) -> GattAttribute {
    attr(
        &GATT_PRIMARY_SERVICE_UUID,
        GATT_PERMIT_READ,
        service as *const GattAttrType as *const u8,
    )
}

const INFO_VALUE: usize = 2;
const MAP_VALUE: usize = 4;
const CONTROL_VALUE: usize = 6;
const PROTOCOL_VALUE: usize = 8;
const INPUT_VALUE: usize = 10;
const INPUT_CCCD: usize = 11;
const INPUT_REFERENCE: usize = 12;
const OUTPUT_VALUE: usize = 14;
const OUTPUT_REFERENCE: usize = 15;
const BOOT_INPUT_VALUE: usize = 17;
const BOOT_INPUT_CCCD: usize = 18;
const BOOT_OUTPUT_VALUE: usize = 20;

static mut HID: [GattAttribute; 21] = [
    primary(&GattAttrType::new_u16(&HID_SERV_UUID)),
    declaration(&GATT_PROP_READ),
    attr(&HID_INFORMATION_UUID, GATT_PERMIT_ENCRYPT_READ, INFO.as_ptr()),
    declaration(&GATT_PROP_READ),
    attr(&REPORT_MAP_UUID, GATT_PERMIT_ENCRYPT_READ, REPORT_MAP.as_ptr()),
    declaration(&GATT_PROP_WRITE_NO_RSP),
    attr(&HID_CTRL_PT_UUID, GATT_PERMIT_ENCRYPT_WRITE, ptr::null()),
    declaration(&(GATT_PROP_READ | GATT_PROP_WRITE_NO_RSP)),
    attr(
        &PROTOCOL_MODE_UUID,
        GATT_PERMIT_ENCRYPT_READ | GATT_PERMIT_ENCRYPT_WRITE,
        ptr::null(),
    ),
    declaration(&(GATT_PROP_READ | GATT_PROP_NOTIFY)),
    attr(&REPORT_UUID, GATT_PERMIT_ENCRYPT_READ, ptr::null()),
    attr(
        &GATT_CLIENT_CHAR_CFG_UUID,
        GATT_PERMIT_READ | GATT_PERMIT_ENCRYPT_WRITE,
        unsafe { REPORT_CCCD.as_ptr() as *const u8 },
    ),
    attr(&GATT_REPORT_REF_UUID, GATT_PERMIT_READ, INPUT_REF.as_ptr()),
    declaration(&(GATT_PROP_READ | GATT_PROP_WRITE | GATT_PROP_WRITE_NO_RSP)),
    attr(
        &REPORT_UUID,
        GATT_PERMIT_ENCRYPT_READ | GATT_PERMIT_ENCRYPT_WRITE,
        ptr::null(),
    ),
    attr(&GATT_REPORT_REF_UUID, GATT_PERMIT_READ, OUTPUT_REF.as_ptr()),
    declaration(&(GATT_PROP_READ | GATT_PROP_NOTIFY)),
    attr(&BOOT_KEY_INPUT_UUID, GATT_PERMIT_ENCRYPT_READ, ptr::null()),
    attr(
        &GATT_CLIENT_CHAR_CFG_UUID,
        GATT_PERMIT_READ | GATT_PERMIT_ENCRYPT_WRITE,
        unsafe { BOOT_CCCD.as_ptr() as *const u8 },
    ),
    declaration(&(GATT_PROP_READ | GATT_PROP_WRITE | GATT_PROP_WRITE_NO_RSP)),
    attr(
        &BOOT_KEY_OUTPUT_UUID,
        GATT_PERMIT_ENCRYPT_READ | GATT_PERMIT_ENCRYPT_WRITE,
        ptr::null(),
    ),
];
static mut BAS: [GattAttribute; 3] = [
    primary(&GattAttrType::new_u16(&BATT_SERV_UUID)),
    declaration(&GATT_PROP_READ),
    attr(&BATT_LEVEL_UUID, GATT_PERMIT_ENCRYPT_READ, BATTERY.as_ptr()),
];
static mut DIS: [GattAttribute; 3] = [
    primary(&GattAttrType::new_u16(&DEVINFO_SERV_UUID)),
    declaration(&GATT_PROP_READ),
    attr(&PNP_ID_UUID, GATT_PERMIT_READ, PNP.as_ptr()),
];

unsafe fn read_bytes(data: &[u8], out: *mut u8, len: *mut u16, offset: u16, max_len: u16) -> u8 {
    let Some(range) = keyboard::read_window(data.len(), offset, max_len) else {
        return ATT_ERR_INVALID_OFFSET;
    };
    *len = range.len() as u16;
    ptr::copy_nonoverlapping(data[range].as_ptr(), out, *len as usize);
    0
}

unsafe extern "C" fn read(
    conn: u16,
    attr: *mut GattAttribute,
    out: *mut u8,
    len: *mut u16,
    offset: u16,
    max_len: u16,
    _method: u8,
) -> u8 {
    let handle = (*attr).handle;
    let data: &[u8] = if handle == BAS[2].handle {
        &BATTERY
    } else if handle == DIS[2].handle {
        &PNP
    } else if handle == HID[INFO_VALUE].handle {
        &INFO
    } else if handle == HID[MAP_VALUE].handle {
        REPORT_MAP
    } else if handle == HID[INPUT_REFERENCE].handle {
        &INPUT_REF
    } else if handle == HID[OUTPUT_REFERENCE].handle {
        &OUTPUT_REF
    } else if handle == HID[PROTOCOL_VALUE].handle {
        slice::from_ref(&STATE.protocol)
    } else if handle == HID[INPUT_VALUE].handle || handle == HID[BOOT_INPUT_VALUE].handle {
        &STATE.report
    } else if handle == HID[OUTPUT_VALUE].handle || handle == HID[BOOT_OUTPUT_VALUE].handle {
        slice::from_ref(&STATE.leds)
    } else if handle == HID[INPUT_CCCD].handle || handle == HID[BOOT_INPUT_CCCD].handle {
        let table = if handle == HID[INPUT_CCCD].handle {
            REPORT_CCCD.as_ptr()
        } else {
            BOOT_CCCD.as_ptr()
        };
        let cfg = GATTServApp::read_char_cfg(conn, table).to_le_bytes();
        return read_bytes(&cfg, out, len, offset, max_len);
    } else {
        return ATT_ERR_ATTR_NOT_FOUND;
    };
    read_bytes(data, out, len, offset, max_len)
}

unsafe extern "C" fn write(
    conn: u16,
    attr: *mut GattAttribute,
    value: *mut u8,
    len: u16,
    offset: u16,
    method: u8,
) -> u8 {
    if method != ATT_WRITE_REQ && method != ATT_WRITE_CMD {
        return ATT_ERR_UNSUPPORTED_REQ;
    }
    let handle = (*attr).handle;
    if handle == HID[INPUT_CCCD].handle || handle == HID[BOOT_INPUT_CCCD].handle {
        return match GATTServApp::process_ccc_write_req(conn, attr, value, len, offset, GATT_CLIENT_CFG_NOTIFY) {
            Ok(()) => 0,
            Err(e) => e.get(),
        };
    }
    if offset != 0 {
        return ATT_ERR_INVALID_OFFSET;
    }
    if len != 1 {
        return ATT_ERR_INVALID_VALUE_SIZE;
    }
    let byte = *value;
    if handle == HID[CONTROL_VALUE].handle {
        if byte > 1 {
            return ATT_ERR_INVALID_VALUE;
        }
        STATE.suspended = byte == 0;
    } else if handle == HID[PROTOCOL_VALUE].handle {
        if byte > 1 {
            return ATT_ERR_INVALID_VALUE;
        }
        STATE.protocol = byte;
        STATE.report = RELEASE;
    } else if handle == HID[OUTPUT_VALUE].handle || handle == HID[BOOT_OUTPUT_VALUE].handle {
        STATE.leds = byte & 0x1f;
    } else {
        return ATT_ERR_ATTR_NOT_FOUND;
    }
    0
}

pub unsafe fn init() {
    static CALLBACKS: gattServiceCBs_t = gattServiceCBs_t {
        pfnReadAttrCB: Some(read),
        pfnWriteAttrCB: Some(write),
        pfnAuthorizeAttrCB: None,
    };
    GATTServApp::init_char_cfg(INVALID_CONNHANDLE, REPORT_CCCD.as_mut_ptr());
    GATTServApp::init_char_cfg(INVALID_CONNHANDLE, BOOT_CCCD.as_mut_ptr());
    GATTServApp::register_service(&mut BAS, GATT_MAX_ENCRYPT_KEY_SIZE, &CALLBACKS).unwrap();
    GATTServApp::register_service(&mut DIS, GATT_MAX_ENCRYPT_KEY_SIZE, &CALLBACKS).unwrap();
    GATTServApp::register_service(&mut HID, GATT_MAX_ENCRYPT_KEY_SIZE, &CALLBACKS).unwrap();
}

pub unsafe fn connected(conn: u16) {
    reset();
    STATE.session.conn = conn;
}

pub unsafe fn reset() {
    STATE.session.conn = INVALID_CONNHANDLE;
    STATE.session.epoch = STATE.session.epoch.wrapping_add(1);
    STATE.protocol = 1;
    STATE.suspended = false;
    STATE.report = RELEASE;
    STATE.leds = 0;
    GATTServApp::init_char_cfg(INVALID_CONNHANDLE, REPORT_CCCD.as_mut_ptr());
    GATTServApp::init_char_cfg(INVALID_CONNHANDLE, BOOT_CCCD.as_mut_ptr());
}

pub fn connection() -> Option<u16> {
    unsafe { (STATE.session.conn != INVALID_CONNHANDLE).then_some(STATE.session.conn) }
}

/// One attempt. Successfully submitted buffers belong to the stack; failures are
/// always freed. Neither caller nor stack may retain borrowed report bytes.
unsafe fn notify(session: Session, handle: u16, report: &[u8; 8]) -> bool {
    if STATE.session != session || linkDB_State(session.conn, LINK_ENCRYPTED) == 0 {
        return false;
    }
    let table = if handle == HID[INPUT_VALUE].handle {
        REPORT_CCCD.as_ptr()
    } else {
        BOOT_CCCD.as_ptr()
    };
    if GATTServApp::read_char_cfg(session.conn, table) & GATT_CLIENT_CFG_NOTIFY == 0 {
        return false;
    }
    let buffer = GATT_bm_alloc(session.conn, ATT_HANDLE_VALUE_NOTI, 8, ptr::null_mut(), 0) as *mut u8;
    if buffer.is_null() {
        return false;
    }
    ptr::copy_nonoverlapping(report.as_ptr(), buffer, 8);
    let notification = attHandleValueNoti_t {
        handle,
        len: 8,
        pValue: buffer,
    };
    match GATT_Notification(session.conn, &notification, 0) {
        Ok(()) => {
            STATE.report = *report;
            true
        }
        Err(_) => {
            GATT_bm_free(
                &mut gattMsg_t {
                    handleValueNoti: notification,
                },
                ATT_HANDLE_VALUE_NOTI,
            );
            false
        }
    }
}

async fn send(session: Session, handle: u16, report: &[u8; 8]) -> bool {
    for _ in 0..500 {
        unsafe {
            if STATE.session != session {
                return false;
            }
            if *report != RELEASE {
                let active = if STATE.protocol == 1 {
                    INPUT_VALUE
                } else {
                    BOOT_INPUT_VALUE
                };
                if STATE.suspended || HID[active].handle != handle {
                    return false;
                }
            }
            if notify(session, handle, report) {
                return true;
            }
        }
        Timer::after_millis(2).await;
    }
    false
}

/// Drop presses before encryption/subscription, never type them upon a later
/// connection. A release stays with the original connection and report handle.
pub async fn tap_a() {
    let (session, handle) = unsafe {
        if STATE.session.conn == INVALID_CONNHANDLE || STATE.suspended {
            return;
        }
        let (index, cfg) = if STATE.protocol == 1 {
            (INPUT_VALUE, REPORT_CCCD.as_ptr())
        } else {
            (BOOT_INPUT_VALUE, BOOT_CCCD.as_ptr())
        };
        if linkDB_State(STATE.session.conn, LINK_ENCRYPTED) == 0
            || GATTServApp::read_char_cfg(STATE.session.conn, cfg) & GATT_CLIENT_CFG_NOTIFY == 0
        {
            return;
        }
        (STATE.session, HID[index].handle)
    };
    if !send(session, handle, &KEY_A).await {
        return;
    }
    Timer::after_millis(40).await;
    if !send(session, handle, &RELEASE).await {
        unsafe {
            if STATE.session == session {
                // Avoid leaving a held key on the host if TX cannot recover.
                let _ = GAPRole_TerminateLink(session.conn);
            }
        }
        log!("HID release failed; terminating original link");
    } else {
        log!("HID A tap sent");
    }
}
