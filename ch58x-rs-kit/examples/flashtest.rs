#![feature(impl_trait_in_assoc_type)]
#![feature(type_alias_impl_trait)]
#![no_std]
#![no_main]

use ch58x_hal::{self as hal};
use hal::gpio::{Level, Output, OutputDrive};
use hal::uart::UartTx;
use ch58x_rs_kit::helpers::blinky;
use ch58x_rs_kit::log;
use ch58x_rs_kit::logger::init as init_logger;

use ch58x_hal::spi::Polarity;
use hal::prelude::*;
use hal::spi::{BitOrder, Spi};
use spi_memory::prelude::*;
use spi_memory::series25::Flash;

#[qingke_rt::entry]
unsafe fn main() -> ! {
    let config = hal::Config::default();
    let p = hal::init(config);

    // let mut ena = Output::new(p.PA4, Level::Low, OutputDrive::_5mA);
    // ena.set_low();

    // let mut but = Input::new(p.PB22, Pull::None);
    let uart = unsafe { hal::peripherals::UART0::steal() };
    let pin_uart = unsafe { hal::peripherals::PB7::steal() };
    let pin_led = unsafe { hal::peripherals::PA8::steal() };
    let led = Output::new(pin_led, Level::Low, OutputDrive::_5mA);

    let serial = UartTx::new(uart, pin_uart, Default::default()).unwrap();
    init_logger(serial);
    log!("\n\n\nHello World!");

    _ = flash_test(p);
    
    blinky(led);
}

pub unsafe fn flash_test(p: hal::Peripherals) -> hal::Peripherals {
    let mut spi_config = hal::spi::Config::default();
    spi_config.frequency = 20.MHz();
    spi_config.bit_order = BitOrder::MsbFirst;
    spi_config.clock_polarity = Polarity::IdleLow;

    let spi_periph = hal::peripherals::SPI0::steal();
    let pin_sck = hal::peripherals::PA13::steal();
    let pin_miso = hal::peripherals::PA14::steal();
    let pin_mosi = hal::peripherals::PA15::steal();
    let pin_cs = hal::peripherals::PA12::steal();

    let spi = Spi::new(spi_periph, pin_sck, pin_miso, pin_mosi, spi_config);
    let cs = Output::new(pin_cs, Level::High, OutputDrive::_5mA);

    let mut flash = Flash::init(spi, cs).unwrap();

    let id = flash.read_jedec_id().unwrap();
    log!("{:?}", id);

    let mut addr = 0;
    const BUF: usize = 32;
    let mut buf = [0; BUF];

    while addr < 1024 {
        flash.read(addr, &mut buf).unwrap_or_else(|e| {
            log!("Error reading flash at {}: {:?}", addr, e);
            return;
        });
        log!("{:?} ", &buf);

        addr += BUF as u32;
    }
    p
}
