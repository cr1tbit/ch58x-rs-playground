#![feature(impl_trait_in_assoc_type)]
#![feature(type_alias_impl_trait)]
#![feature(generic_const_exprs)]
#![no_std]
#![no_main]

use ch58x_hal as hal;
use embassy_executor::Spawner;
use hal::gpio::{Level, Output, OutputDrive};
use hal::peripherals;
use hal::uart::UartTx;
use hal::ble;
use ch58x_rs_kit::logger::init as init_logger;
use ch58x_rs_kit::{get_configured_rtc, log, tmos_mainloop};
use ch58x_rs_kit::sr_led_driver::SrLedDriver;
use ledface_layout::Layout;

use ch58x_rs_kit::ble_periph::{common_init, devinfo_init, peripheral};
use ch58x_rs_kit::ble_periph::current_time_service::current_time_init;

use embassy_time::Duration;
use embassy_time::Timer;
use ch58x_hal::gpio::AnyPin;

const TEST_LAYOUT_JSON: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../ledface_layout/test/test.json"
));
const COMPILED_LAYOUT_BYTES: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/watchface_layout.bin"));

type WatchLayout = Layout<48, 4, 48>;

fn load_layout() -> Option<WatchLayout> {
    match Layout::from_bytes(COMPILED_LAYOUT_BYTES) {
        Ok(layout) => Some(layout),
        Err(_) => Layout::from_json(TEST_LAYOUT_JSON).ok(),
    }
}

#[embassy_executor::task]
async fn async_blink(pin: AnyPin) {
    let mut led = Output::new(pin, Level::Low, OutputDrive::_5mA);

    loop {
        led.set_high();
        Timer::after(Duration::from_millis(150)).await;
        led.set_low();
        Timer::after(Duration::from_millis(150)).await;
    }
}

#[embassy_executor::task]
async fn sr_driver_task(mut sr_driver: SrLedDriver<'static, peripherals::SPI0, 48>) -> ! {
    log!("Shift register driver task started");
    sr_driver.enable_output();
    let mut layout = load_layout();
    if let Some(ref layout) = layout {
        log!(
            "Loaded LED layout: {} LEDs, {} entities",
            layout.led_count(),
            layout.entities.len()
        );
    } else {
        log!("Layout unavailable, using direct LED writes");
    }

    let mut frame = [0u8; 48];
    let mut text_angle: u16 = 0;
    let mut wave_angle: u16 = 0;
    let mut direct_index: usize = 0;
    let mut iteration: u32 = 0;
    loop {
        iteration = iteration.wrapping_add(1);

        if let Some(layout_ref) = layout.as_ref() {
            frame.fill(0);

            if let Some(text) = layout_ref.entity("Text") {
                text.set_degrees(text_angle, &mut frame);
            }
            if let Some(wave) = layout_ref.entity("Wave") {
                wave.set_degrees(wave_angle, &mut frame);
            }

            text_angle = (text_angle + 5) % 360;
            wave_angle = (wave_angle + 5) % 360;

            if let Err(err) = sr_driver.write(0, &frame) {
                log!("Frame write error {:?}, switching to direct mode", err);
                layout = None;
                continue;
            }
        // } else {
            // sr_driver.clear();
            // sr_driver
            //     .set_led(direct_index, 0xFF)
            //     .expect("direct LED index out of range");

            // direct_index = (direct_index + 1) % sr_driver.len();
        }

        log!("SR iteration {}", iteration);
        sr_driver.update();
        Timer::after(Duration::from_millis(200)).await;
    }
}

#[embassy_executor::main(entry = "qingke_rt::entry")]
async fn main(spawner: Spawner) -> ! {
    let mut config = hal::Config::default();
    config.clock.use_pll_60mhz().enable_lse();
    let p = hal::init(config);
    hal::embassy::init();

    let mut ena = Output::new(p.PB13, Level::Low, OutputDrive::_5mA);
    ena.set_high();

    let serial = UartTx::new(p.UART0, p.PB7, Default::default()).unwrap();
    init_logger(serial);
    log!("\n\n\nArtsyWatch Firmware Started!");

    // Shift register control pins for LED matrix column driving
    // SCK - PA13 (SPI Clock)
    // DAT - PA14 (SPI MOSI/Data)
    // OE  - PA8  (Output Enable, directly GPIO-controlled)
    // LAT - PA9  (Latch, directly GPIO-controlled)

    // Initialize shift register LED driver from pins
    let mut sr_driver =
        SrLedDriver::<_, 48>::new_from_pins::<false, _, _, _, _>(p.SPI0, p.PA13, p.PA14, p.PA8, p.PA9);

    log!("Shift register driver initialized");
    log!("  SPI: SCK=PA13, DAT=PA14");
    log!("  GPIO: OE=PA8, LAT=PA9");

    // Quick connectivity check before starting the main task.
    // sr_driver.test_write(0xFF);
    // sr_driver.test_write(0x00);
    // log!("Starting shift register background task...");
    // spawner.spawn(sr_driver_task(sr_driver)).unwrap();

    let rtc = get_configured_rtc();

    log!("System Clocks: {}", hal::sysctl::clocks().hclk);
    log!("ChipID: 0x{:02x}", hal::signature::get_chip_id());
    log!("RTC datetime: {}", rtc.now());

    log!("BLE Lib Version: {}", ble::lib_version());

    let mut ble_config = ble::Config::default();
    ble_config.pa_config = None;
    ble_config.mac_addr = [0x21, 0x37, 0x04, 0x20, 0x69, 0x96].into();
    let (task_id, sub) = hal::ble::init(ble_config).unwrap();
    log!("BLE hal task id: {}", task_id);

    // loop {
    //     Timer::after(Duration::from_secs(1)).await;
    //     log!("Heartbeat: main alive");
    // }

    unsafe {
        common_init();
        devinfo_init();
        current_time_init();
    }

    // Main_Circulation
    spawner.spawn(tmos_mainloop()).unwrap();

    // Application code
    peripheral(spawner, task_id, sub, rtc, None).await
}
