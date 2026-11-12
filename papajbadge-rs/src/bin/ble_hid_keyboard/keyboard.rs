//! Transport-independent keyboard data and sampled button debounce.

pub const RELEASE: [u8; 8] = [0; 8];
pub const KEY_A: [u8; 8] = [0, 0, 0x04, 0, 0, 0, 0, 0];
pub const REPORT_ID: u8 = 1;

// One keyboard application, Report ID 1: eight input bytes and one LED byte.
// GATT Report values omit the ID byte; Report Reference descriptors carry it.
pub const REPORT_MAP: &[u8] = &[
    0x05, 0x01, 0x09, 0x06, 0xa1, 0x01, 0x85, REPORT_ID, 0x05, 0x07, 0x19, 0xe0, 0x29, 0xe7, 0x15, 0x00, 0x25, 0x01,
    0x75, 0x01, 0x95, 0x08, 0x81, 0x02, // modifier bits
    0x75, 0x08, 0x95, 0x01, 0x81, 0x01, // reserved byte
    0x95, 0x06, 0x15, 0x00, 0x25, 0x65, 0x19, 0x00, 0x29, 0x65, 0x81, 0x00, // six key usages
    0x05, 0x08, 0x19, 0x01, 0x29, 0x05, 0x75, 0x01, 0x95, 0x05, 0x91, 0x02, // five keyboard LEDs
    0x75, 0x03, 0x95, 0x01, 0x91, 0x01, 0xc0, // LED padding, end
];

/// Call every 5 ms; require 20 ms of stable samples. A boot-held BOOT button
/// must first be released. No repeats, and no event on the release edge.
pub struct Button {
    candidate: bool,
    samples: u8,
    stable: bool,
    armed: bool,
}

impl Button {
    pub const fn new() -> Self {
        Self {
            candidate: true,
            samples: 0,
            stable: true,
            armed: false,
        }
    }

    pub fn sample(&mut self, pressed: bool) -> bool {
        if pressed != self.candidate {
            self.candidate = pressed;
            self.samples = 1;
            return false;
        }
        self.samples = self.samples.saturating_add(1);
        if self.samples < 4 {
            return false;
        }
        let changed = self.stable != pressed;
        self.stable = pressed;
        if !pressed {
            self.armed = true;
            false
        } else if changed && self.armed {
            self.armed = false;
            true
        } else {
            false
        }
    }
}

pub fn read_window(total: usize, offset: u16, max_len: u16) -> Option<core::ops::Range<usize>> {
    let start = offset as usize;
    (start <= total).then(|| start..core::cmp::min(total, start + max_len as usize))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples(button: &mut Button, pressed: bool, count: usize) -> usize {
        (0..count).filter(|_| button.sample(pressed)).count()
    }

    #[test]
    fn boot_hold_noise_and_one_tap_per_press() {
        let mut button = Button::new();
        assert_eq!(samples(&mut button, true, 200), 0);
        assert_eq!(samples(&mut button, false, 4), 0);
        for pressed in [true, false, true, false, true, false] {
            assert!(!button.sample(pressed));
        }
        assert_eq!(samples(&mut button, true, 200), 1);
        assert_eq!(samples(&mut button, false, 4), 0);
        assert_eq!(samples(&mut button, true, 4), 1);
    }

    #[test]
    fn report_map_has_eight_input_bytes_and_one_output_byte() {
        // Interpret HID short-item global size/count/ID and main input/output.
        let (mut size, mut count, mut id) = (0, 0, 0);
        let (mut input, mut output) = (0, 0);
        let mut at = 0;
        while at < REPORT_MAP.len() {
            let prefix = REPORT_MAP[at];
            let len = match prefix & 3 {
                3 => 4,
                n => n as usize,
            };
            let value = REPORT_MAP.get(at + 1).copied().unwrap_or(0) as usize;
            match prefix & 0xfc {
                0x74 => size = value,
                0x94 => count = value,
                0x84 => id = value,
                0x80 => {
                    assert_eq!(id, REPORT_ID as usize);
                    input += size * count;
                }
                0x90 => {
                    assert_eq!(id, REPORT_ID as usize);
                    output += size * count;
                }
                _ => {}
            }
            at += len + 1;
        }
        assert_eq!((input, output), (64, 8));
        assert_eq!(KEY_A[2], 0x04);
        assert_eq!(RELEASE, [0; 8]);
    }

    #[test]
    fn report_map_reads_reassemble_at_mtu_23() {
        let mut reconstructed = std::vec::Vec::new();
        let mut offset = 0;
        loop {
            let range = read_window(REPORT_MAP.len(), offset, 22).unwrap();
            if range.is_empty() {
                break;
            }
            reconstructed.extend_from_slice(&REPORT_MAP[range.clone()]);
            offset = range.end as u16;
        }
        assert_eq!(reconstructed, REPORT_MAP);
        assert!(read_window(REPORT_MAP.len(), REPORT_MAP.len() as u16 + 1, 22).is_none());
    }
}
