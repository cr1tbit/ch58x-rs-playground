#![allow(static_mut_refs)]

use {ch58x_hal as hal};
use core::{ptr, slice};

use ch58x_hal::ble::gatt_uuid;
use ch58x_hal::rtc::Rtc;
use ch58x_hal::soft_reset;
use hal::ble::ffi::*;
use hal::ble::gatt::*;
use hal::ble::gattservapp::*;
use super::{APP_CHANNEL, AppEvent};

use crate::log;

// Bluetooth Assigned Numbers
// Service: Current Time Service (CTS) 0x1805
// Characteristic: Current Time 0x2A2B

#[derive(Copy, Clone, Debug)]
pub struct CurrentTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hours: u8,
    pub minutes: u8,
    pub seconds: u8,
    pub day_of_week: u8,   // 1=Monday, 7=Sunday
    pub fractions256: u8,  // 1/256th of a second
    pub adjust_reason: u8, // bitfield per spec
}

impl Default for CurrentTime {
    fn default() -> Self {
        // Arbitrary default mock time
        Self {
            year: 2025,
            month: 1,
            day: 1,
            hours: 0,
            minutes: 0,
            seconds: 0,
            day_of_week: 4, // Thu
            fractions256: 0,
            adjust_reason: 0,
        }
    }
}

impl CurrentTime {
    pub fn to_bytes(&self) -> [u8; 10] {
        let mut buf = [0u8; 10];
        buf[0..2].copy_from_slice(&self.year.to_le_bytes());
        buf[2] = self.month;
        buf[3] = self.day;
        buf[4] = self.hours;
        buf[5] = self.minutes;
        buf[6] = self.seconds;
        buf[7] = self.day_of_week;
        buf[8] = self.fractions256;
        buf[9] = self.adjust_reason;
        buf
    }
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 10 { return None; }
        Some(Self {
            year: u16::from_le_bytes([data[0], data[1]]),
            month: data[2],
            day: data[3],
            hours: data[4],
            minutes: data[5],
            seconds: data[6],
            day_of_week: data[7],
            fractions256: data[8],
            adjust_reason: data[9],
        })
    }

    pub fn from_datetime(dt: &hal::rtc::DateTime) -> Self {
        Self {
            year: dt.year,
            month: dt.month,
            day: dt.day,
            hours: dt.hour,
            minutes: dt.minute,
            seconds: dt.second,
            day_of_week: 3, //it is wenesday my dudes
            fractions256: 0,
            adjust_reason: 0,
        }
    }

    pub fn to_datetime(&self) -> hal::rtc::DateTime {
        hal::rtc::DateTime {
            year: (if self.year < 2020 { 2037 } else { self.year }),
            month: (if self.month == 0 { 1 } else { self.month }),
            day: (if self.day == 0 { 1 } else { self.day }),
            hour: self.hours,
            minute: self.minutes,
            second: self.seconds,
            millisecond: 0,
        }
    }
}

// Mock RTC storage. Replace with a real RTC backend when available.
static mut CURRENT_TIME_VALUE: CurrentTime = CurrentTime {
    year: 2025,
    month: 1,
    day: 1,
    hours: 0,
    minutes: 0,
    seconds: 0,
    day_of_week: 4,
    fractions256: 0,
    adjust_reason: 0,
};

// Client Characteristic Configuration table for notifications
static mut CTS_CLIENT_CHARCFG: [gattCharCfg_t; 4] = unsafe { core::mem::zeroed() };

// Public mock RTC API
pub unsafe fn rtc_get_time() -> CurrentTime {
    let rtc = Rtc {};
    let now = rtc.now();

    CURRENT_TIME_VALUE = CurrentTime::from_datetime(&now);
    log!("CUR TIME ARRAY: {:X?}", CURRENT_TIME_VALUE.to_bytes());
    CURRENT_TIME_VALUE
}

pub fn rtc_set_time(ct: CurrentTime) {
    // let mut rtc = Rtc {};
    let now = ct.to_datetime();
    
    // rtc.set_datatime(now);
    // using RTC here seems to crash something lol.
    // the time should probably be pushed to some queue and set in another context
    log!("MOCK SET TODO: {:02}:{:02}:{:02} {:02}/{:02}/{}",
            now.hour, now.minute, now.second, now.month, now.day, now.year);

    let _ = APP_CHANNEL.try_send(AppEvent::TimeSet(now));
    // //reset board 
    // unsafe{soft_reset();}
}

// GATT Attribute Table
// Primary Service, Characteristic Declaration, Characteristic Value
static mut CURRENT_TIME_ATTR_TABLE: [GattAttribute; 4] = [
    // Current Time Service
    GattAttribute {
        type_: GattAttrType::PRIMARY_SERVICE,
        permissions: GATT_PERMIT_READ,
        handle: 0,
        value: &GattAttrType {
            len: ATT_BT_UUID_SIZE,
            uuid: &gatt_uuid::CURRENT_TIME_SERV_UUID as *const _ as _,
        } as *const _ as _,
    },
    // Current Time Declaration
    GattAttribute {
        type_: GattAttrType::CHARACTERISTIC,
        permissions: GATT_PERMIT_READ,
        handle: 0,
        value: &(GATT_PROP_READ | GATT_PROP_WRITE | GATT_PROP_NOTIFY) as *const _ as _,
    },
    // Current Time Value
    GattAttribute {
        type_: GattAttrType::new_u16(&gatt_uuid::CURRENT_TIME_UUID),
        permissions: GATT_PERMIT_READ | GATT_PERMIT_WRITE,
        handle: 0,
        value: ptr::null(), // value provided via callbacks
    },
    // Client Characteristic Configuration Descriptor (CCCD)
    GattAttribute {
        type_: GattAttrType::CLIENT_CHAR_CFG,
        permissions: GATT_PERMIT_READ | GATT_PERMIT_WRITE,
        handle: 0,
        value: unsafe { CTS_CLIENT_CHARCFG.as_ptr() as _ },
    },
];

pub unsafe fn current_time_init() {
    // Initialize CCCD table
    GATTServApp::init_char_cfg(INVALID_CONNHANDLE, CTS_CLIENT_CHARCFG.as_mut_ptr());

    unsafe extern "C" fn on_read_attr(
        _conn_handle: u16,
        attr: *mut GattAttribute,
        value: *mut u8,
        plen: *mut u16,
        offset: u16,
        _max_len: u16,
        _method: u8,
    ) -> u8 {
        // No long reads supported
        if offset > 0 { return ATT_ERR_ATTR_NOT_LONG; }

        let uuid = *((*attr).type_.uuid as *const u16);
        log!("CTS on_read_attr UUID: 0x{:04x}", uuid);

        match uuid {
            gatt_uuid::CURRENT_TIME_UUID => {
                let ct: CurrentTime = rtc_get_time();
                let bytes = ct.to_bytes();
                *plen = bytes.len() as _;
                core::ptr::copy(bytes.as_ptr(), value, bytes.len());
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
        log!("CTS on_write_attr UUID: 0x{:04x}", uuid);

        match uuid {
            gatt_uuid::CURRENT_TIME_UUID => unsafe {
                let val = slice::from_raw_parts(value, len as usize);
                if let Some(ct) = CurrentTime::from_bytes(val) {
                    // CURRENT_TIME_VALUE = ct;
                    rtc_set_time(ct);
                    0
                } else {
                    ATT_ERR_INVALID_VALUE_SIZE
                }
            },
            // CCCD writes to enable/disable notifications
            gatt_uuid::GATT_CLIENT_CHAR_CFG_UUID => {
                let status = GATTServApp::process_ccc_write_req(
                    conn_handle,
                    attr,
                    value,
                    len,
                    offset,
                    GATT_CLIENT_CFG_NOTIFY,
                );
                // if status.is_ok() { 0 } else { status.err().unwrap_or(0x01) }
                if status.is_ok() { 0 } else { 0x0D }
            }
            _ => ATT_ERR_ATTR_NOT_FOUND,
        }
    }

    static CTS_SERVICE_CB: gattServiceCBs_t = gattServiceCBs_t {
        pfnReadAttrCB: Some(on_read_attr),
        pfnWriteAttrCB: Some(on_write_attr),
        pfnAuthorizeAttrCB: None,
    };

    // Register service
    GATTServApp::register_service(
        &mut CURRENT_TIME_ATTR_TABLE[..],
        GATT_MAX_ENCRYPT_KEY_SIZE,
        &CTS_SERVICE_CB,
    )
    .unwrap();
}
