#![allow(static_mut_refs)]

use {ch58x_hal as hal};
use core::{ptr, slice};

use hal::ble::ffi::*;
use hal::ble::gatt::*;
use hal::ble::gattservapp::*;
use hal::ble::gatt_uuid;
use embassy_time::{Duration, Timer};

use crate::log; // import logging macro

// UUIDs (Assigned Numbers)
pub const HID_SERV_UUID: u16 = 0x1812; // Human Interface Device
const HID_INFORMATION_UUID: u16 = 0x2A4A;
const HID_REPORT_MAP_UUID: u16 = 0x2A4B;
const HID_CONTROL_POINT_UUID: u16 = 0x2A4C;
const HID_REPORT_UUID: u16 = 0x2A4D;
const HID_PROTOCOL_MODE_UUID: u16 = 0x2A4E;

// Descriptors
const GATT_REPORT_REF_DESC_UUID: u16 = 0x2908; // Report Reference

// Keyboard reports
pub const HID_KEYBOARD_INPUT_REPORT_LEN: usize = 8; // modifier, reserved, 6 keys
pub const HID_KEYBOARD_OUTPUT_REPORT_LEN: usize = 1; // LED bitmap (5 bits used)

// CCCD storage
pub static mut HID_INPUT_CCCD: [gattCharCfg_t; 4] = unsafe { core::mem::zeroed() };

// HID static values
static HID_INFORMATION: [u8; 4] = [
    0x11, 0x01, // bcdHID = 0x0111 (HID v1.11)
    0x00,       // bCountryCode = 0 (not localized)
    0x03,       // Flags: bit0 RemoteWake, bit1 NormallyConnectable
];

// Minimal keyboard Report Map (Report Protocol)
// Matches an 8-byte input report and 1-byte output report for LEDs
#[rustfmt::skip]
static HID_REPORT_MAP: &[u8] = &[
    0x05, 0x01,       // Usage Page (Generic Desktop)
    0x09, 0x06,       // Usage (Keyboard)
    0xA1, 0x01,       // Collection (Application)
    // Input report (8 bytes)
    0x05, 0x07,       //   Usage Page (Key Codes)
    0x19, 0xE0,       //   Usage Minimum (224)
    0x29, 0xE7,       //   Usage Maximum (231)
    0x15, 0x00,       //   Logical Minimum (0)
    0x25, 0x01,       //   Logical Maximum (1)
    0x75, 0x01,       //   Report Size (1)
    0x95, 0x08,       //   Report Count (8)
    0x81, 0x02,       //   Input (Data,Var,Abs) ; Modifier byte
    0x95, 0x01,       //   Report Count (1)
    0x75, 0x08,       //   Report Size (8)
    0x81, 0x03,       //   Input (Const,Var,Abs) ; Reserved
    0x95, 0x05,       //   Report Count (5)
    0x75, 0x01,       //   Report Size (1)
    0x05, 0x08,       //   Usage Page (LEDs)
    0x19, 0x01,       //   Usage Minimum (1)
    0x29, 0x05,       //   Usage Maximum (5)
    0x91, 0x02,       //   Output (Data,Var,Abs) ; LED report
    0x95, 0x01,       //   Report Count (1)
    0x75, 0x03,       //   Report Size (3)
    0x91, 0x03,       //   Output (Const,Var,Abs) ; LED padding
    0x95, 0x06,       //   Report Count (6)
    0x75, 0x08,       //   Report Size (8)
    0x15, 0x00,       //   Logical Minimum (0)
    0x25, 0x65,       //   Logical Maximum (101)
    0x05, 0x07,       //   Usage Page (Key Codes)
    0x19, 0x00,       //   Usage Minimum (0)
    0x29, 0x65,       //   Usage Maximum (101)
    0x81, 0x00,       //   Input (Data,Array)
    0xC0,             // End Collection
];

// Protocol Mode: 1 = Report Protocol (we don't support Boot Protocol here)
static mut PROTOCOL_MODE: u8 = 1;

// Report Reference values
// [Report ID, Report Type] where type: 1=Input, 2=Output, 3=Feature
static REPORT_REF_INPUT: [u8; 2] = [0, 1];
static REPORT_REF_OUTPUT: [u8; 2] = [0, 2];

// Last values (for reads)
static mut LAST_INPUT_REPORT: [u8; HID_KEYBOARD_INPUT_REPORT_LEN] = [0; HID_KEYBOARD_INPUT_REPORT_LEN];
static mut LAST_OUTPUT_REPORT: u8 = 0; // LED state bitmap

// Attribute table indices for convenience
const IDX_HID_INPUT_VAL: usize = 10; // value attribute index of Input Report
const IDX_HID_OUTPUT_VAL: usize = 14; // value attribute index of Output Report

// GATT Attribute Table for HID service
static mut HID_ATTR_TABLE: [GattAttribute; 16] = [
    // 0 - Primary Service: HID
    GattAttribute {
        type_: GattAttrType::PRIMARY_SERVICE,
        permissions: GATT_PERMIT_READ,
        handle: 0,
        value: &GattAttrType { len: ATT_BT_UUID_SIZE, uuid: &HID_SERV_UUID as *const _ as _ } as *const _ as _,
    },
    // 1 - Characteristic: HID Information (read)
    GattAttribute {
        type_: GattAttrType::CHARACTERISTIC,
        permissions: GATT_PERMIT_READ,
        handle: 0,
        value: &(GATT_PROP_READ) as *const _ as _,
    },
    // 2 - Value: HID Information (2A4A)
    GattAttribute {
        type_: GattAttrType::new_u16(&HID_INFORMATION_UUID),
        permissions: GATT_PERMIT_READ,
        handle: 0,
        value: ptr::null(), // serve via read callback for visibility
    },
    // 3 - Characteristic: Report Map (read)
    GattAttribute {
        type_: GattAttrType::CHARACTERISTIC,
        permissions: GATT_PERMIT_READ,
        handle: 0,
        value: &(GATT_PROP_READ) as *const _ as _,
    },
    // 4 - Value: Report Map (2A4B)
    GattAttribute {
        type_: GattAttrType::new_u16(&HID_REPORT_MAP_UUID),
        permissions: GATT_PERMIT_READ,
        handle: 0,
        value: ptr::null(), // serve via read callback, supports long reads
    },
    // 5 - Characteristic: HID Control Point (write without response)
    GattAttribute {
        type_: GattAttrType::CHARACTERISTIC,
        permissions: GATT_PERMIT_READ,
        handle: 0,
        value: &(GATT_PROP_WRITE_NO_RSP) as *const _ as _,
    },
    // 6 - Value: HID Control Point (2A4C)
    GattAttribute {
        type_: GattAttrType::new_u16(&HID_CONTROL_POINT_UUID),
        permissions: GATT_PERMIT_WRITE,
        handle: 0,
        value: ptr::null(),
    },
    // 7 - Characteristic: Protocol Mode (read, write without response)
    GattAttribute {
        type_: GattAttrType::CHARACTERISTIC,
        permissions: GATT_PERMIT_READ,
        handle: 0,
        value: &((GATT_PROP_READ) | (GATT_PROP_WRITE_NO_RSP)) as *const _ as _,
    },
    // 8 - Value: Protocol Mode (2A4E)
    GattAttribute {
        type_: GattAttrType::new_u16(&HID_PROTOCOL_MODE_UUID),
        permissions: GATT_PERMIT_READ | GATT_PERMIT_WRITE,
        handle: 0,
        value: unsafe { &PROTOCOL_MODE as *const _ as _ },
    },
    // 9 - Characteristic: Input Report (read, notify)
    GattAttribute {
        type_: GattAttrType::CHARACTERISTIC,
        permissions: GATT_PERMIT_READ,
        handle: 0,
        value: &((GATT_PROP_READ) | (GATT_PROP_NOTIFY)) as *const _ as _,
    },
    // 10 - Value: Input Report (2A4D)
    GattAttribute {
        type_: GattAttrType::new_u16(&HID_REPORT_UUID),
        permissions: GATT_PERMIT_READ,
        handle: 0,
        value: ptr::null(), // provided by read callback
    },
    // 11 - CCCD for Input Report (2902)
    GattAttribute {
        type_: GattAttrType::CLIENT_CHAR_CFG,
        permissions: GATT_PERMIT_READ | GATT_PERMIT_WRITE,
        handle: 0,
        value: unsafe { HID_INPUT_CCCD.as_ptr() as _ },
    },
    // 12 - Report Reference for Input Report (2908)
    GattAttribute {
        type_: GattAttrType::new_u16(&GATT_REPORT_REF_DESC_UUID),
        permissions: GATT_PERMIT_READ,
        handle: 0,
        value: REPORT_REF_INPUT.as_ptr(),
    },
    // 13 - Characteristic: Output Report (read, write, write no rsp)
    GattAttribute {
        type_: GattAttrType::CHARACTERISTIC,
        permissions: GATT_PERMIT_READ,
        handle: 0,
        value: &((GATT_PROP_READ) | (GATT_PROP_WRITE) | (GATT_PROP_WRITE_NO_RSP)) as *const _ as _,
    },
    // 14 - Value: Output Report (2A4D)
    GattAttribute {
        type_: GattAttrType::new_u16(&HID_REPORT_UUID),
        permissions: GATT_PERMIT_READ | GATT_PERMIT_WRITE,
        handle: 0,
        value: ptr::null(), // via callbacks
    },
    // 15 - Report Reference for Output Report (2908)
    GattAttribute {
        type_: GattAttrType::new_u16(&GATT_REPORT_REF_DESC_UUID),
        permissions: GATT_PERMIT_READ,
        handle: 0,
        value: REPORT_REF_OUTPUT.as_ptr(),
    },
];

pub unsafe fn hid_init() {
    // Initialize CCCD
    GATTServApp::init_char_cfg(INVALID_CONNHANDLE, HID_INPUT_CCCD.as_mut_ptr());

    unsafe extern "C" fn on_read_attr(
        _conn_handle: u16,
        attr: *mut GattAttribute,
        value: *mut u8,
        plen: *mut u16,
        offset: u16,
        max_len: u16,
        _method: u8,
    ) -> u8 {

        let uuid = *((*attr).type_.uuid as *const u16);
        log!("HID on_read_attr UUID: 0x{:04x} handle:{} offset:{} max:{}", uuid, (*attr).handle, offset, max_len);

        match uuid {
            HID_INFORMATION_UUID => {
                // Short, fixed-size read
                if offset > 0 { return ATT_ERR_ATTR_NOT_LONG; }
                *plen = HID_INFORMATION.len() as _;
                ptr::copy(HID_INFORMATION.as_ptr(), value, HID_INFORMATION.len());
                0
            }
            HID_REPORT_MAP_UUID => {
                // Support long read with offset; handle MTU-sized chunks
                let total = HID_REPORT_MAP.len() as u16;
                if offset >= total { *plen = 0; return 0; }
                let remaining = total - offset;
                let to_copy = core::cmp::min(remaining, max_len);
                *plen = to_copy;
                unsafe {
                    ptr::copy(HID_REPORT_MAP.as_ptr().add(offset as usize), value, to_copy as usize);
                }
                0
            }
            HID_REPORT_UUID => {
                if offset > 0 { return ATT_ERR_ATTR_NOT_LONG; }
                // Distinguish Input vs Output by handle (robust across copies)
                if (*attr).handle == unsafe { HID_ATTR_TABLE[IDX_HID_INPUT_VAL].handle } {
                    *plen = HID_KEYBOARD_INPUT_REPORT_LEN as _;
                    ptr::copy(LAST_INPUT_REPORT.as_ptr(), value, HID_KEYBOARD_INPUT_REPORT_LEN);
                    0
                } else if (*attr).handle == unsafe { HID_ATTR_TABLE[IDX_HID_OUTPUT_VAL].handle } {
                    *plen = 1;
                    ptr::copy(&LAST_OUTPUT_REPORT as *const _ as _, value, 1);
                    0
                } else {
                    ATT_ERR_ATTR_NOT_FOUND
                }
            }
            HID_PROTOCOL_MODE_UUID => {
                if offset > 0 { return ATT_ERR_ATTR_NOT_LONG; }
                *plen = 1;
                ptr::copy(&PROTOCOL_MODE as *const _ as _, value, 1);
                0
            }
            _ => ATT_ERR_ATTR_NOT_FOUND,
        }
    }

    unsafe extern "C" fn on_write_attr(
        conn_handle: u16,
        attr: *mut GattAttribute,
        value: *mut u8,
        len: u16,
        offset: u16,
        _method: u8,
    ) -> u8 {
        if offset > 0 { return ATT_ERR_ATTR_NOT_LONG; }
        let uuid = *((*attr).type_.uuid as *const u16);
        let slice = unsafe { slice::from_raw_parts(value, len as usize) };
        log!(
            "HID on_write_attr UUID: 0x{:04x} handle:{} len:{} val:{:02x?}",
            uuid,
            (*attr).handle,
            len,
            slice
        );

        match uuid {
            HID_CONTROL_POINT_UUID => {
                if slice.len() != 1 { return ATT_ERR_INVALID_VALUE_SIZE; }
                0
            }
            HID_PROTOCOL_MODE_UUID => {
                if slice.len() != 1 { return ATT_ERR_INVALID_VALUE_SIZE; }
                if slice[0] == 1 { PROTOCOL_MODE = 1; 0 } else { ATT_ERR_UNSUPPORTED_REQ }
            }
            HID_REPORT_UUID => {
                if (*attr).handle == unsafe { HID_ATTR_TABLE[IDX_HID_OUTPUT_VAL].handle } {
                    if slice.len() < 1 { return ATT_ERR_INVALID_VALUE_SIZE; }
                    LAST_OUTPUT_REPORT = slice[0];
                    0
                } else {
                    ATT_ERR_ATTR_NOT_FOUND
                }
            }
            gatt_uuid::GATT_CLIENT_CHAR_CFG_UUID => {
                let status = GATTServApp::process_ccc_write_req(
                    conn_handle,
                    attr,
                    value,
                    len,
                    offset,
                    GATT_CLIENT_CFG_NOTIFY,
                );
                if status.is_ok() { 0 } else { 0x0D }
            }
            _ => ATT_ERR_ATTR_NOT_FOUND,
        }
    }

    static HID_SERVICE_CB: gattServiceCBs_t = gattServiceCBs_t {
        pfnReadAttrCB: Some(on_read_attr),
        pfnWriteAttrCB: Some(on_write_attr),
        pfnAuthorizeAttrCB: None,
    };

    GATTServApp::register_service(&mut HID_ATTR_TABLE[..], GATT_MAX_ENCRYPT_KEY_SIZE, &HID_SERVICE_CB)
        .unwrap();
}

// Send an 8-byte Keyboard Input Report (modifier, reserved, 6 keycodes)
pub unsafe fn hid_notify_input_report(conn_handle: u16, report: &[u8; HID_KEYBOARD_INPUT_REPORT_LEN]) {
    // Cache last report (for reads)
    ptr::copy(report.as_ptr(), LAST_INPUT_REPORT.as_mut_ptr(), HID_KEYBOARD_INPUT_REPORT_LEN);

    // Check if notifications are enabled
    let ccc = GATTServApp::read_char_cfg(conn_handle, HID_INPUT_CCCD.as_ptr());
    if ccc != 0x01 { 
        log!("no notif");
        return; }

    // Build notification
    let mut noti: gattMsg_t = gattMsg_t {
        handleValueNoti: attHandleValueNoti_t { handle: 0, len: HID_KEYBOARD_INPUT_REPORT_LEN as u16, pValue: core::ptr::null_mut() },
    };

    noti.handleValueNoti.pValue = GATT_bm_alloc(0, ATT_HANDLE_VALUE_NOTI, HID_KEYBOARD_INPUT_REPORT_LEN as u16, ptr::null_mut(), 0) as _;
    if noti.handleValueNoti.pValue.is_null() { 
        log!("null");return; }

    noti.handleValueNoti.handle = unsafe { HID_ATTR_TABLE[IDX_HID_INPUT_VAL].handle };
    noti.handleValueNoti.len = HID_KEYBOARD_INPUT_REPORT_LEN as u16;
    ptr::copy(report.as_ptr(), noti.handleValueNoti.pValue, HID_KEYBOARD_INPUT_REPORT_LEN);

    let _ = GATT_Notification(conn_handle, &noti.handleValueNoti, 0);
    log!("done");
}

// Helper to send a single key press (press + release)
pub unsafe fn hid_send_keypress(conn_handle: u16, keycode: u8, modifiers: u8) {
    let mut rpt = [0u8; HID_KEYBOARD_INPUT_REPORT_LEN];
    rpt[0] = modifiers; // modifier bits
    rpt[2] = keycode;   // first key
    hid_notify_input_report(conn_handle, &rpt);

    // Release (all zeros)
    let rpt_release = [0u8; HID_KEYBOARD_INPUT_REPORT_LEN];
    hid_notify_input_report(conn_handle, &rpt_release);
}

#[embassy_executor::task]
pub async unsafe fn keypresser(conn_handle: u16) {
    log!("Starting key presser task");
    loop 
    {        
        log!("Sending keypress");
        Timer::after(Duration::from_millis((1000 as u32).into())).await;
        hid_send_keypress(conn_handle, 0x04, 0x02); // 'a' with left shift
    }        
}