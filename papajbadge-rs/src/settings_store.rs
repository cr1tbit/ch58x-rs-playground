//! Tiny wear-levelled settings store in the CH58x DataFlash ("EEPROM") area.
//!
//! One 256-byte erase page is split into 32 slots of 8 bytes. Every save appends a record
//! to the next slot; the page is only erased once it is full, so a page erase happens once
//! every 32 saves. On load, the last valid record in the page wins.
//!
//! Record layout: `[MAGIC, payload[0..6], crc8(MAGIC..payload)]`.

use ch58x_hal::isp;

pub const PAYLOAD_LEN: usize = 6;
const RECORD_LEN: usize = PAYLOAD_LEN + 2;
const PAGE_LEN: usize = isp::EEPROM_MIN_ERASE_SIZE as usize;
const SLOTS: usize = PAGE_LEN / RECORD_LEN;
const MAGIC: u8 = 0xA5;

pub struct SettingsStore {
    /// DataFlash offset of the page (must be 256-byte aligned).
    base: u32,
    next_slot: usize,
}

fn crc8(data: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &byte in data {
        crc ^= byte;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 { (crc << 1) ^ 0x07 } else { crc << 1 };
        }
    }
    crc
}

fn encode(payload: &[u8; PAYLOAD_LEN]) -> [u8; RECORD_LEN] {
    let mut record = [0u8; RECORD_LEN];
    record[0] = MAGIC;
    record[1..=PAYLOAD_LEN].copy_from_slice(payload);
    record[RECORD_LEN - 1] = crc8(&record[..RECORD_LEN - 1]);
    record
}

fn decode(record: &[u8]) -> Option<[u8; PAYLOAD_LEN]> {
    if record[0] != MAGIC || crc8(&record[..RECORD_LEN - 1]) != record[RECORD_LEN - 1] {
        return None;
    }
    let mut payload = [0u8; PAYLOAD_LEN];
    payload.copy_from_slice(&record[1..=PAYLOAD_LEN]);
    Some(payload)
}

impl SettingsStore {
    /// Scan the page at `base` and return the store plus the most recently saved payload.
    pub fn load(base: u32) -> (Self, Option<[u8; PAYLOAD_LEN]>) {
        let mut page = [0u8; PAGE_LEN];
        critical_section::with(|_| isp::eeprom_read(base, &mut page));

        // Records are written sequentially from slot 0, so the first invalid slot ends the log.
        let mut last = None;
        let mut next_slot = 0;
        for (slot, record) in page.chunks_exact(RECORD_LEN).enumerate() {
            match decode(record) {
                Some(payload) => {
                    last = Some(payload);
                    next_slot = slot + 1;
                }
                None => break,
            }
        }

        (Self { base, next_slot }, last)
    }

    /// Append `payload` as the newest record. Blocks the CPU for a few ms (longer on page erase).
    pub fn save(&mut self, payload: &[u8; PAYLOAD_LEN]) -> bool {
        let record = encode(payload);

        if self.next_slot >= SLOTS || !self.write_slot(self.next_slot, &record) {
            // Page full, or the slot was not clean (first use / corruption): start over.
            critical_section::with(|_| isp::eeprom_erase(self.base, PAGE_LEN as u32));
            self.next_slot = 0;
            if !self.write_slot(0, &record) {
                return false;
            }
        }

        self.next_slot += 1;
        true
    }

    fn write_slot(&self, slot: usize, record: &[u8; RECORD_LEN]) -> bool {
        let addr = self.base + (slot * RECORD_LEN) as u32;
        let mut readback = [0u8; RECORD_LEN];
        critical_section::with(|_| {
            isp::eeprom_write(addr, record);
            isp::eeprom_read(addr, &mut readback);
        });
        readback == *record
    }
}
