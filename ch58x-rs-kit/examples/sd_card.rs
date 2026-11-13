//! microSD over SPI0 with `embedded-sdmmc`: card init, data checks, directory listing, file
//! read and read speed, reported on the UART log (PB7, 115200). Written for the muzg board
//! (SCLK PA13, MOSI PA14, MISO PA15, CS PA12 with 10k pull-ups, card detect PB4).
//!
//! `cargo build --release --example sd_card` (`CLOCK=60` for 60 MHz instead of 20 MHz,
//! `SD_DIV=3` for a slower SPI clock: SPI runs at hclk / SD_DIV after init, default 2).
//!
//! What it does:
//! - Initialises the card at 400 kHz (with retries), then switches SPI to hclk / SD_DIV.
//! - Compares block 0 read slow and fast, prints the MBR partition and the FAT boot sector.
//! - Reads the first file of the root directory by hand (first cluster from its directory
//!   entry, contiguous clusters assumed) and prints it as text.
//! - Measures raw single-block (CMD17) and 8-block (CMD18) read speed.
//! - Lists the root directory and reads the largest file (or a `.WAV`) through embedded-sdmmc.
//!
//! Findings on a cheap 512 MB card (2026-11-13):
//! - ~175-190 KB/s at 20 MHz / SPI 10 MHz, ~200-230 KB/s at 60 MHz / SPI 20 MHz: limited by
//!   the card's access delay per block (~2.5 ms), not by SPI or the CPU.
//! - embedded-sdmmc 0.8.2 decides the FAT type by cluster count, as the FAT spec says. A card
//!   formatted "FAT32" with too few clusters (mkfs.fat -F 32 on 512 MB: 32 KB clusters, 15331
//!   clusters) is FAT16 by that rule, and the root directory comes out empty. Format such
//!   cards with `mkfs.fat -F 16`, or `mkfs.fat -F 32 -s 8`. The hand-made read above works
//!   either way.
//! - embedded-sdmmc 0.8.2 reports a wrong size for version-2 standard-capacity cards ("SD2"):
//!   it decodes their CSD with the version-2 layout. Reading is unaffected.
//! - The HAL's FIFO-based `Spi::blocking_read` doesn't wait for incoming data; this example
//!   exchanges each byte on the SPI0 data register instead.

#![no_std]
#![no_main]
#![feature(impl_trait_in_assoc_type)]

use core::convert::Infallible;

use ch58x_hal as hal;
use ch58x_rs_kit::log;
use ch58x_rs_kit::logger::init as init_logger;
use embassy_time::Instant;
use embedded_hal_local::spi::{ErrorType, Operation, SpiDevice};
use embedded_sdmmc::{Block, BlockDevice, BlockIdx, Mode, SdCard, TimeSource, Timestamp, VolumeIdx, VolumeManager};
use hal::delay::CycleDelay;
use hal::gpio::{Input, Level, Output, OutputDrive, Pull};
use hal::peripherals;
use hal::prelude::Hertz;
use hal::spi::{self, Spi};
use hal::uart::UartTx;

#[qingke_rt::entry]
fn main() -> ! {
    hal::embassy::Executor::new().run(|spawner| spawner.must_spawn(run()))
}

#[qingke_rt::interrupt]
fn Exception() {
    ch58x_rs_kit::fatal::halt(core::format_args!("CPU exception"))
}

/// SPI0 + chip select as an `embedded-hal` `SpiDevice`. Each byte is one full-duplex exchange on
/// the SPI0 data register (the HAL's FIFO read doesn't wait for incoming data).
struct SdSpi<'d> {
    _spi: Spi<'d, peripherals::SPI0>,
    cs: Output<'d, peripherals::PA12>,
}

impl SdSpi<'_> {
    #[inline(always)]
    fn xfer(&mut self, out: u8) -> u8 {
        let rb = unsafe { &*hal::pac::SPI0::PTR };
        rb.buffer().write(|w| unsafe { w.bits(out) });
        while !rb.int_flag().read().free().bit_is_set() {}
        rb.buffer().read().bits()
    }

    /// SPI clock = hclk / `div` (2..=255).
    fn set_clock_div(&mut self, div: u8) {
        let rb = unsafe { &*hal::pac::SPI0::PTR };
        rb.ctrl_cfg().modify(|_, w| w.mst_dly_en().bit(div == 2));
        rb.clock_div().write(|w| unsafe { w.clock_div().bits(div) });
    }

    /// ≥74 clocks with CS high, before the card is talked to (left to the caller by
    /// embedded-sdmmc).
    fn wake_card(&mut self) {
        self.cs.set_high();
        for _ in 0..10 {
            self.xfer(0xFF);
        }
    }
}

impl ErrorType for SdSpi<'_> {
    type Error = Infallible;
}

impl SpiDevice<u8> for SdSpi<'_> {
    fn transaction(&mut self, operations: &mut [Operation<'_, u8>]) -> Result<(), Infallible> {
        {
            let rb = unsafe { &*hal::pac::SPI0::PTR };
            rb.ctrl_mod().modify(|_, w| w.fifo_dir().clear_bit());
        }
        self.cs.set_low();
        for op in operations {
            match op {
                Operation::Read(buf) => {
                    for b in buf.iter_mut() {
                        *b = self.xfer(0xFF);
                    }
                }
                Operation::Write(buf) => {
                    for &b in buf.iter() {
                        self.xfer(b);
                    }
                }
                Operation::Transfer(read, write) => {
                    for i in 0..read.len().max(write.len()) {
                        let r = self.xfer(write.get(i).copied().unwrap_or(0xFF));
                        if let Some(slot) = read.get_mut(i) {
                            *slot = r;
                        }
                    }
                }
                Operation::TransferInPlace(buf) => {
                    for b in buf.iter_mut() {
                        *b = self.xfer(*b);
                    }
                }
                Operation::DelayNs(ns) => {
                    embedded_hal_local::delay::DelayNs::delay_ns(&mut CycleDelay, *ns);
                }
            }
        }
        self.cs.set_high();
        Ok(())
    }
}

/// No real-time clock for file timestamps (read-only use anyway).
struct NoClock;
impl TimeSource for NoClock {
    fn get_timestamp(&self) -> Timestamp {
        Timestamp { year_since_1970: 56, zero_indexed_month: 0, zero_indexed_day: 0, hours: 0, minutes: 0, seconds: 0 }
    }
}

/// KB/s for `bytes` read in `since`.
fn kbps(bytes: u32, since: Instant) -> u32 {
    let us = since.elapsed().as_micros().max(1);
    (bytes as u64 * 1_000_000 / 1024 / us) as u32
}

#[embassy_executor::task]
async fn run() {
    let mut config = hal::Config::default();
    match option_env!("CLOCK") {
        Some("60") => {
            config.clock.use_pll_60mhz();
        }
        _ => {
            config.clock.use_pll_20mhz();
        }
    }
    config.clock.enable_lse();
    let mut p = hal::init(config);
    hal::embassy::init();
    init_logger(UartTx::new(p.UART0, p.PB7, Default::default()).unwrap());
    let hclk = hal::sysctl::clocks().hclk.to_Hz();
    let div: u8 = option_env!("SD_DIV").and_then(|d| d.parse().ok()).unwrap_or(2);
    log!("\n\nsd_card: hclk {} MHz, SPI after init {} kHz", hclk / 1_000_000, hclk / div as u32 / 1000);

    let detect = Input::new(&mut p.PB4, Pull::None).is_high();
    log!("card detect (PB4): {}", if detect { "card present" } else { "no card" });

    let mut spi_config = spi::Config::default();
    spi_config.frequency = Hertz::from_raw(400_000);
    let spi = Spi::new::<false>(p.SPI0, p.PA13, p.PA14, p.PA15, spi_config);
    let cs = Output::new(p.PA12, Level::High, OutputDrive::_5mA);
    let mut device = SdSpi { _spi: spi, cs };
    device.wake_card();

    let card = SdCard::new(device, CycleDelay);
    let t = Instant::now();
    // A reset in the middle of a card command can leave the card busy: retry the init.
    let mut attempt = 0;
    let bytes = loop {
        attempt += 1;
        match card.num_bytes() {
            Ok(bytes) => break bytes,
            Err(e) if attempt < 3 => {
                log!("card init attempt {} failed: {:?}, retrying", attempt, e);
                card.mark_card_uninit();
                card.spi(|spi| spi.wake_card());
            }
            Err(e) => {
                log!("card init failed: {:?}", e);
                return;
            }
        }
    };
    log!("card: {:?}, {} MB, init {} ms", card.get_card_type(), bytes / 1_000_000, t.elapsed().as_millis());
    // Data check: block 0 at 400 kHz vs fast, MBR partition entry, volume boot sector.
    let mut slow = [Block::new()];
    let mut fast = [Block::new()];
    let _ = card.read(&mut slow, BlockIdx(0), "slow");
    card.spi(|spi| spi.set_clock_div(div));
    let _ = card.read(&mut fast, BlockIdx(0), "fast");
    let same = slow[0].contents == fast[0].contents;
    log!("block 0 at 400 kHz vs fast: {}", if same { "identical" } else { "DIFFERENT" });
    let b = &slow[0].contents;
    let part = &b[0x1BE..0x1CE];
    let lba = u32::from_le_bytes([part[8], part[9], part[10], part[11]]);
    let sectors = u32::from_le_bytes([part[12], part[13], part[14], part[15]]);
    log!("MBR: sig {:02x}{:02x}, partition 0: boot {:02x} type {:02x} start LBA {} sectors {} ({} MB)",
        b[510], b[511], part[0], part[4], lba, sectors, sectors / 2048);
    let mut vbr = [Block::new()];
    let reserved_and_fats = |v: &[u8]| u16::from_le_bytes([v[14], v[15]]) as u32 + v[16] as u32 * u32::from_le_bytes([v[36], v[37], v[38], v[39]]);
    if card.read(&mut vbr, BlockIdx(lba), "vbr").is_ok() {
        let v = &vbr[0].contents;
        log!("VBR: jump {:02x}{:02x}{:02x} oem {:?} bytes/sector {} sectors/cluster {} reserved {} fats {} sectors/fat32 {} root cluster {} sig {:02x}{:02x} fstype {:?}",
            v[0], v[1], v[2], core::str::from_utf8(&v[3..11]).unwrap_or("?"),
            u16::from_le_bytes([v[11], v[12]]), v[13], u16::from_le_bytes([v[14], v[15]]), v[16],
            u32::from_le_bytes([v[36], v[37], v[38], v[39]]), u32::from_le_bytes([v[44], v[45], v[46], v[47]]),
            v[510], v[511], core::str::from_utf8(&v[82..90]).unwrap_or("?"));
        // first sector of the root directory (cluster 2 = start of the data area)
        let root_lba = lba + reserved_and_fats(v);
        let mut dir = [Block::new()];
        if card.read(&mut dir, BlockIdx(root_lba), "root").is_ok() {
            // Read the first regular file in the root directory by hand (first cluster from the
            // directory entry; contiguous clusters assumed, fine for a small file): this works
            // even when embedded-sdmmc misjudges the FAT type.
            let spc = v[13] as u32;
            let entry = dir[0].contents.chunks(32).find(|e| e[0] != 0 && e[0] != 0xE5 && e[11] & 0x18 == 0 && e[11] != 0x0F);
            if let Some(e) = entry {
                let cluster = (u16::from_le_bytes([e[20], e[21]]) as u32) << 16 | u16::from_le_bytes([e[26], e[27]]) as u32;
                let size = u32::from_le_bytes([e[28], e[29], e[30], e[31]]);
                let file_lba = root_lba + (cluster - 2) * spc;
                log!("raw FAT: file {:?} cluster {} size {} at LBA {}", core::str::from_utf8(&e[0..11]).unwrap_or("?"), cluster, size, file_lba);
                let mut data = [Block::new()];
                let mut left = size;
                let mut lba_i = file_lba;
                let mut line = [0u8; 100];
                let mut n = 0usize;
                while left > 0 && card.read(&mut data, BlockIdx(lba_i), "file").is_ok() {
                    let take = left.min(512) as usize;
                    for &b in &data[0].contents[..take] {
                        if b == b'\n' || n == line.len() {
                            log!("  | {}", core::str::from_utf8(&line[..n]).unwrap_or("?"));
                            n = 0;
                            if b == b'\n' { continue; }
                        }
                        line[n] = if (0x20..0x7f).contains(&b) { b } else { b'.' };
                        n += 1;
                    }
                    left -= take as u32;
                    lba_i += 1;
                }
                if n > 0 {
                    log!("  | {}", core::str::from_utf8(&line[..n]).unwrap_or("?"));
                }
            }
        }
    }

    // raw reads
    let mut blocks = [Block::new(), Block::new(), Block::new(), Block::new(), Block::new(), Block::new(), Block::new(), Block::new()];
    const RAW_BLOCKS: u32 = 512; // 256 KiB
    let t = Instant::now();
    for i in 0..RAW_BLOCKS {
        if let Err(e) = card.read(&mut blocks[..1], BlockIdx(i), "cmd17") {
            log!("single-block read {} failed: {:?}", i, e);
            return;
        }
    }
    log!("raw CMD17 (1 block per command): {} KB/s", kbps(RAW_BLOCKS * 512, t));
    let t = Instant::now();
    for i in (0..RAW_BLOCKS).step_by(8) {
        if let Err(e) = card.read(&mut blocks, BlockIdx(i), "cmd18") {
            log!("multi-block read {} failed: {:?}", i, e);
            return;
        }
    }
    log!("raw CMD18 (8 blocks per command): {} KB/s", kbps(RAW_BLOCKS * 512, t));

    // FAT
    let mut mgr: VolumeManager<_, _, 4, 4, 1> = VolumeManager::new_with_limits(card, NoClock, 0);
    let mut volume = match mgr.open_volume(VolumeIdx(0)) {
        Ok(v) => v,
        Err(e) => {
            log!("open volume 0 failed: {:?}", e);
            return;
        }
    };
    let mut root = match volume.open_root_dir() {
        Ok(d) => d,
        Err(e) => {
            log!("open root dir failed: {:?}", e);
            return;
        }
    };
    let mut pick: Option<(embedded_sdmmc::ShortFileName, u32, bool)> = None;
    let mut count = 0;
    log!("root directory:");
    let listing = root.iterate_dir(|entry| {
        count += 1;
        let dir = entry.attributes.is_directory();
        if count <= 40 {
            log!("  {:12} {:>10} {}", entry.name, entry.size, if dir { "<DIR>" } else { "" });
        }
        if dir || entry.attributes.is_volume() {
            return;
        }
        let wav = entry.name.extension() == b"WAV";
        let better = match &pick {
            None => true,
            Some((_, size, was_wav)) => (wav && !was_wav) || (wav == *was_wav && entry.size > *size),
        };
        if better {
            pick = Some((entry.name.clone(), entry.size, wav));
        }
    });
    if let Err(e) = &listing {
        log!("iterate_dir failed: {:?}", e);
    }
    log!("{} entries", count);
    let Some((name, size, wav)) = pick else {
        log!("no file to read");
        return;
    };

    let mut file = match root.open_file_in_dir(&name, Mode::ReadOnly) {
        Ok(f) => f,
        Err(e) => {
            log!("open {} failed: {:?}", name, e);
            return;
        }
    };
    let mut buf = [0u8; 4096];
    let mut header = [0u8; 44];
    let mut head = [0u8; 240];
    let mut head_len = 0;
    let limit = size.min(4 * 1024 * 1024);
    let mut total = 0u32;
    let t = Instant::now();
    while total < limit {
        match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if total == 0 {
                    let h = n.min(44);
                    header[..h].copy_from_slice(&buf[..h]);
                    head_len = n.min(head.len());
                    head[..head_len].copy_from_slice(&buf[..head_len]);
                }
                total += n as u32;
            }
            Err(e) => {
                log!("read failed after {} bytes: {:?}", total, e);
                break;
            }
        }
    }
    let speed = kbps(total, t);
    log!("file {} ({} bytes): read {} bytes through FAT at {} KB/s", name, size, total, speed);
    // the start of the file as text, line by line (non-printable bytes as '.')
    log!("first {} bytes:", head_len);
    for line in head[..head_len].split(|&b| b == b'\n') {
        let mut text = [0u8; 80];
        let n = line.len().min(text.len());
        for (t, &b) in text.iter_mut().zip(line) {
            *t = if (0x20..0x7f).contains(&b) { b } else { b'.' };
        }
        log!("  | {}", core::str::from_utf8(&text[..n]).unwrap_or("?"));
    }

    if wav && &header[0..4] == b"RIFF" && &header[8..12] == b"WAVE" {
        let channels = u16::from_le_bytes([header[22], header[23]]);
        let rate = u32::from_le_bytes([header[24], header[25], header[26], header[27]]);
        let byte_rate = u32::from_le_bytes([header[28], header[29], header[30], header[31]]);
        let bits = u16::from_le_bytes([header[34], header[35]]);
        let need = byte_rate / 1024;
        log!("WAV: {} Hz, {} bit, {} channel(s): needs {} KB/s, reading gives {} KB/s -> {}x real time",
            rate, bits, channels, need, speed, speed / need.max(1));
    }
    log!("done");
}
