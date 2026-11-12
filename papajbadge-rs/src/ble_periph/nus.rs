//! Nordic UART Service. Call all GAP/GATT operations from executor/TMOS thread mode.

#![allow(static_mut_refs)]

use core::cell::Cell;
use core::{ptr, slice};

use ch58x_hal::ble::ffi::*;
use ch58x_hal::ble::gap::GAPRole_SetParameter;
use ch58x_hal::ble::gatt::*;
use ch58x_hal::ble::gattservapp::*;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::channel::Channel;
use embassy_time::Timer;

// UUIDs in ATT byte order: 6E40000x-B5A3-F393-E0A9-E50E24DCCA9E.
pub static SERVICE_UUID: [u8; 16] = [
    0x9e, 0xca, 0xdc, 0x24, 0x0e, 0xe5, 0xa9, 0xe0, 0x93, 0xf3, 0xa3, 0xb5, 1, 0, 0x40, 0x6e,
];
static RX_UUID: [u8; 16] = [
    0x9e, 0xca, 0xdc, 0x24, 0x0e, 0xe5, 0xa9, 0xe0, 0x93, 0xf3, 0xa3, 0xb5, 2, 0, 0x40, 0x6e,
];
static TX_UUID: [u8; 16] = [
    0x9e, 0xca, 0xdc, 0x24, 0x0e, 0xe5, 0xa9, 0xe0, 0x93, 0xf3, 0xa3, 0xb5, 3, 0, 0x40, 0x6e,
];

// The current HAL uses ATT MTU 23. Larger application messages are fragmented.
pub const PACKET_LEN: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Session {
    conn: u16,
    epoch: u32,
}

impl Session {
    pub fn is_current(self) -> bool {
        STATE.lock(|s| {
            let s = s.get();
            s.initialized && s.session == self
        })
    }
}

#[derive(Clone, Copy)]
struct State {
    initialized: bool,
    session: Session,
    resync: bool,
}

static STATE: Mutex<CriticalSectionRawMutex, Cell<State>> = Mutex::new(Cell::new(State {
    initialized: false,
    session: Session {
        conn: INVALID_CONNHANDLE,
        epoch: 0,
    },
    resync: false,
}));

pub struct Packet {
    pub session: Session,
    data: [u8; PACKET_LEN],
    len: usize,
}
impl Packet {
    pub fn bytes(&self) -> &[u8] {
        &self.data[..self.len]
    }
}
static RX: Channel<CriticalSectionRawMutex, Packet, 8> = Channel::new();
static mut TX_CCCD: [gattCharCfg_t; 4] = unsafe { core::mem::zeroed() };
static mut ADVERT: [u8; 31] = [0; 31];

static mut ATTRS: [GattAttribute; 6] = [
    GattAttribute {
        type_: GattAttrType::PRIMARY_SERVICE,
        permissions: GATT_PERMIT_READ,
        handle: 0,
        value: &GattAttrType {
            len: 16,
            uuid: SERVICE_UUID.as_ptr(),
        } as *const _ as _,
    },
    GattAttribute {
        type_: GattAttrType::CHARACTERISTIC,
        permissions: GATT_PERMIT_READ,
        handle: 0,
        value: &(GATT_PROP_WRITE | GATT_PROP_WRITE_NO_RSP) as *const _ as _,
    },
    GattAttribute {
        type_: GattAttrType {
            len: 16,
            uuid: RX_UUID.as_ptr(),
        },
        permissions: GATT_PERMIT_WRITE,
        handle: 0,
        value: ptr::null(),
    },
    GattAttribute {
        type_: GattAttrType::CHARACTERISTIC,
        permissions: GATT_PERMIT_READ,
        handle: 0,
        value: &GATT_PROP_NOTIFY as *const _ as _,
    },
    GattAttribute {
        type_: GattAttrType {
            len: 16,
            uuid: TX_UUID.as_ptr(),
        },
        permissions: 0,
        handle: 0,
        value: ptr::null(),
    },
    GattAttribute {
        type_: GattAttrType::CLIENT_CHAR_CFG,
        permissions: GATT_PERMIT_READ | GATT_PERMIT_WRITE,
        handle: 0,
        value: unsafe { TX_CCCD.as_ptr() as _ },
    },
];

unsafe extern "C" fn read(
    conn: u16,
    attr: *mut GattAttribute,
    value: *mut u8,
    len: *mut u16,
    offset: u16,
    max_len: u16,
    _method: u8,
) -> u8 {
    if (*attr).handle != ATTRS[5].handle {
        return ATT_ERR_ATTR_NOT_FOUND;
    }
    if offset != 0 {
        return ATT_ERR_INVALID_OFFSET;
    }
    if max_len < 2 {
        return ATT_ERR_INVALID_VALUE_SIZE;
    }
    let cfg = GATTServApp::read_char_cfg(conn, TX_CCCD.as_ptr()).to_le_bytes();
    ptr::copy_nonoverlapping(cfg.as_ptr(), value, 2);
    *len = 2;
    0
}

unsafe extern "C" fn write(
    conn: u16,
    attr: *mut GattAttribute,
    value: *mut u8,
    len: u16,
    offset: u16,
    _method: u8,
) -> u8 {
    if (*attr).handle == ATTRS[5].handle {
        return match GATTServApp::process_ccc_write_req(conn, attr, value, len, offset, GATT_CLIENT_CFG_NOTIFY) {
            Ok(()) => 0,
            Err(code) => code.get(),
        };
    }
    if (*attr).handle != ATTRS[2].handle {
        return ATT_ERR_ATTR_NOT_FOUND;
    }
    if offset != 0 {
        return ATT_ERR_INVALID_OFFSET;
    }
    if len as usize > PACKET_LEN {
        return ATT_ERR_INVALID_VALUE_SIZE;
    }
    if len == 0 {
        return 0;
    }
    let mut bytes = slice::from_raw_parts(value, len as usize);
    STATE.lock(|state| {
        let mut s = state.get();
        if s.session.conn != conn {
            return ATT_ERR_INVALID_HANDLE;
        }
        // If an unacknowledged write overran the queue, discard the damaged line.
        // A delimiter reestablishes framing; never execute a truncated command.
        if s.resync {
            if let Some(end) = bytes.iter().position(|b| *b == b'\n' || *b == b'\r') {
                bytes = &bytes[end + 1..];
                s.resync = false;
                state.set(s);
            } else {
                return ATT_ERR_INSUFFICIENT_RESOURCES;
            }
        }
        if bytes.is_empty() {
            return 0;
        }
        let mut packet = Packet {
            session: s.session,
            data: [0; PACKET_LEN],
            len: bytes.len(),
        };
        packet.data[..bytes.len()].copy_from_slice(bytes);
        if RX.try_send(packet).is_err() {
            RX.clear();
            s.session.epoch = s.session.epoch.wrapping_add(1);
            s.resync = true;
            state.set(s);
            return ATT_ERR_INSUFFICIENT_RESOURCES;
        }
        0
    })
}

/// Register NUS and append its UUID to the existing advertisement (30 bytes total).
/// Must run once, after `common_init` and before starting the peripheral.
pub unsafe fn init() {
    static CALLBACKS: gattServiceCBs_t = gattServiceCBs_t {
        pfnReadAttrCB: Some(read),
        pfnWriteAttrCB: Some(write),
        pfnAuthorizeAttrCB: None,
    };
    GATTServApp::init_char_cfg(INVALID_CONNHANDLE, TX_CCCD.as_mut_ptr());
    GATTServApp::register_service(&mut ATTRS, GATT_MAX_ENCRYPT_KEY_SIZE, &CALLBACKS).unwrap();
    let len = super::ADVERT_DATA.len();
    assert!(len + 18 <= ADVERT.len());
    ADVERT[..len].copy_from_slice(super::ADVERT_DATA);
    ADVERT[len] = 17;
    ADVERT[len + 1] = GAP_ADTYPE_128BIT_MORE;
    ADVERT[len + 2..len + 18].copy_from_slice(&SERVICE_UUID);
    GAPRole_SetParameter(GAPROLE_ADVERT_DATA, (len + 18) as _, ADVERT.as_ptr() as _).unwrap();
    STATE.lock(|state| {
        let mut s = state.get();
        s.initialized = true;
        state.set(s);
    });
}

pub(super) fn connected(conn: u16) {
    STATE.lock(|state| {
        let mut s = state.get();
        if !s.initialized {
            return;
        }
        s.session = Session {
            conn,
            epoch: s.session.epoch.wrapping_add(1),
        };
        s.resync = false;
        state.set(s);
        RX.clear();
        unsafe {
            GATTServApp::init_char_cfg(INVALID_CONNHANDLE, TX_CCCD.as_mut_ptr());
        }
    });
}

pub(super) fn disconnected(conn: u16) {
    STATE.lock(|state| {
        let mut s = state.get();
        if !s.initialized || s.session.conn != conn {
            return;
        }
        s.session = Session {
            conn: INVALID_CONNHANDLE,
            epoch: s.session.epoch.wrapping_add(1),
        };
        s.resync = false;
        state.set(s);
        RX.clear();
        unsafe {
            GATTServApp::init_char_cfg(conn, TX_CCCD.as_mut_ptr());
        }
    });
}

pub async fn receive() -> Packet {
    RX.receive().await
}

#[derive(Debug)]
pub enum SendError {
    Disconnected,
    NotSubscribed,
    Busy,
    Stack(u8),
}

/// The stack owns successfully submitted buffers. Every failed submission is freed
/// before retrying, including during disconnect or notification backpressure.
pub async fn send(session: Session, bytes: &[u8]) -> Result<(), SendError> {
    for chunk in bytes.chunks(PACKET_LEN) {
        let mut sent = false;
        // The negotiated connection interval can exceed 100 ms. A burst can fill
        // all five controller buffers, so wait across several connection events.
        // This task yields while waiting; the LED/PWM task keeps running.
        for _ in 0..1000 {
            if !session.is_current() {
                return Err(SendError::Disconnected);
            }
            unsafe {
                if GATTServApp::read_char_cfg(session.conn, TX_CCCD.as_ptr()) & GATT_CLIENT_CFG_NOTIFY == 0 {
                    return Err(SendError::NotSubscribed);
                }
                let buffer = GATT_bm_alloc(
                    session.conn,
                    ATT_HANDLE_VALUE_NOTI,
                    chunk.len() as _,
                    ptr::null_mut(),
                    0,
                ) as *mut u8;
                if !buffer.is_null() {
                    ptr::copy_nonoverlapping(chunk.as_ptr(), buffer, chunk.len());
                    let notification = attHandleValueNoti_t {
                        handle: ATTRS[4].handle,
                        len: chunk.len() as _,
                        pValue: buffer,
                    };
                    match GATT_Notification(session.conn, &notification, 0) {
                        Ok(()) => {
                            sent = true;
                            break;
                        }
                        Err(code) => {
                            let mut message = gattMsg_t {
                                handleValueNoti: notification,
                            };
                            GATT_bm_free(&mut message, ATT_HANDLE_VALUE_NOTI);
                            // HCI buffer unavailable, allocation failure, or pending.
                            if !matches!(code.get(), 0x04 | 0x13 | 0x16) {
                                return Err(SendError::Stack(code.get()));
                            }
                        }
                    }
                }
            }
            Timer::after_millis(2).await;
        }
        if !sent {
            return Err(SendError::Busy);
        }
    }
    Ok(())
}
