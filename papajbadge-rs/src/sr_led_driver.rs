//! Shift Register LED Matrix Column Driver
//!
//! Provides a buffered shift-register driver suitable for running from either a task or ISR.

use core::cell::{Cell, UnsafeCell};

use ch58x_hal as hal;
use hal::gpio::{AnyPin, Level, Output, OutputDrive, Pin};
use hal::spi::{Config as SpiConfig, Instance as SpiInstance, Spi};
use hal::Peripheral;

/// Errors that can occur when manipulating the LED buffer.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum SrDriverError {
    /// Attempted to write outside of the configured buffer range.
    OutOfBounds,
}

/// Convenient alias for driver specific results.
pub type Result<T> = core::result::Result<T, SrDriverError>;

/// Shift register LED driver for matrix column control.
///
/// The driver keeps an internal per-LED buffer (`N` bytes) as well as a packed bit-buffer
/// (`N/8` bytes) that is written to the shift register on [`Self::update`].
pub struct SrLedDriver<'d, SPI: SpiInstance, const N: usize>
where
    [(); (N + 7) / 8]: ,
{
    spi: Spi<'d, SPI>,
    oe: Output<'d, AnyPin>,
    lat: Output<'d, AnyPin>,
    frame: [Cell<u8>; N],
    spi_buf: UnsafeCell<[u8; (N + 7) / 8]>,
}

impl<'d, SPI: SpiInstance, const N: usize> SrLedDriver<'d, SPI, N>
where
    [(); (N + 7) / 8]: ,
{
    /// Create a new shift register LED driver from already configured peripherals.
    pub fn new(
        spi: Spi<'d, SPI>,
        oe: Output<'d, AnyPin>,
        lat: Output<'d, AnyPin>,
    ) -> Self {
        Self {
            spi,
            oe,
            lat,
            frame: core::array::from_fn(|_| Cell::new(0)),
            spi_buf: UnsafeCell::new([0u8; (N + 7) / 8]),
        }
    }

    /// Create a new shift register LED driver from raw pins.
    pub fn new_from_pins<const REMAP: bool, SCK, DAT, OE, LAT>(
        spi_peri: impl Peripheral<P = SPI> + 'd,
        sck: impl Peripheral<P = SCK> + 'd,
        dat: impl Peripheral<P = DAT> + 'd,
        oe_pin: impl Peripheral<P = OE> + 'd,
        lat_pin: impl Peripheral<P = LAT> + 'd,
    ) -> Self
    where
        SCK: hal::spi::SckPin<SPI, REMAP> + 'd,
        DAT: hal::spi::MosiPin<SPI, REMAP> + 'd,
        OE: Pin + 'd,
        LAT: Pin + 'd,
    {
        let spi_config = SpiConfig::default();
        let spi = Spi::new_txonly::<REMAP>(spi_peri, sck, dat, spi_config);

        // OE typically active low, start disabled (high)
        let oe = Output::new(oe_pin, Level::High, OutputDrive::_5mA).degrade();
        // LAT low initially
        let lat = Output::new(lat_pin, Level::Low, OutputDrive::_5mA).degrade();

        Self::new(spi, oe, lat)
    }

    /// Number of logical LEDs supported by this driver instance.
    pub const fn len(&self) -> usize {
        N
    }

    /// Write data into the buffered frame at the requested offset.
    ///
    /// Values are stored verbatim and interpreted during [`Self::update`].
    pub fn write(&self, offset: usize, data: &[u8]) -> Result<()> {
        let end = offset.checked_add(data.len()).ok_or(SrDriverError::OutOfBounds)?;
        if end > N {
            return Err(SrDriverError::OutOfBounds);
        }
        for (idx, value) in data.iter().enumerate() {
            self.frame[offset + idx].set(*value);
        }
        Ok(())
    }

    /// Clear the buffered frame.
    pub fn clear(&self) {
        for led in &self.frame {
            led.set(0);
        }
    }

    /// Set a single LED value in the current frame.
    pub fn set_led(&self, index: usize, value: u8) -> Result<()> {
        if index >= N {
            return Err(SrDriverError::OutOfBounds);
        }
        self.frame[index].set(value);
        Ok(())
    }

    /// Enable the shift register outputs (active low OE).
    pub fn enable_output(&mut self) {
        self.oe.set_low();
    }

    /// Disable the shift register outputs (active low OE).
    pub fn disable_output(&mut self) {
        self.oe.set_high();
    }

    /// Pulse the latch to transfer shift register contents to outputs.
    fn latch(&mut self) {
        self.lat.set_high();
        // Simple busy-wait delay (~1ms at 60MHz)
        for _ in 0..600_0 {
            core::hint::black_box(());
        }
        self.lat.set_low();
    }

    /// Commit the currently buffered frame to the shift registers.
    ///
    /// For now every non-zero value is treated as LED "on".
    pub fn update(&mut self) {
        self.disable_output();
        self.pack_frame();
        let data = unsafe { &*self.spi_buf.get() };
        let _ = self.spi.blocking_write(data);
        self.latch();
        self.enable_output();
    }

    /// Shift the buffered frame into the registers and latch it, leaving `/OE` untouched.
    ///
    /// Much faster than [`Self::update`], so it can be used several times per PWM period
    /// (e.g. to time-multiplex LED groups).
    pub fn load_frame(&mut self) {
        use embedded_hal_local::delay::DelayNs;

        self.pack_frame();
        let data = unsafe { &*self.spi_buf.get() };
        let _ = self.spi.blocking_write(data);
        // blocking_write returns once the FIFO is drained; let the last byte finish shifting
        // out (8 us at the default 1 MHz) before latching.
        let mut delay = hal::delay::CycleDelay;
        delay.delay_us(10);
        self.lat.set_high();
        delay.delay_us(1);
        self.lat.set_low();
    }

    /// Helper used by [`Self::update`] to pack cells into the SPI buffer.
    fn pack_frame(&self) {
        let spi_buf = unsafe { &mut *self.spi_buf.get() };
        spi_buf.fill(0);
        for (idx, led) in self.frame.iter().enumerate() {
            if led.get() != 0 {
                let byte = idx / 8;
                let bit = idx % 8;
                spi_buf[byte] |= 1 << bit;
            }
        }
        for chunk in spi_buf.chunks_exact_mut(2) {
            chunk.swap(0, 1);
        }
    }

    /// Test helper: fill the buffer with a repeating pattern and update once.
    pub fn test_write(&mut self, pattern: u8) {
        for led in &self.frame {
            led.set(pattern);
        }
        self.update();
    }
}
