#![feature(impl_trait_in_assoc_type)]
#![feature(type_alias_impl_trait)]

#![no_std]
#![no_main]


use {ch58x_hal as hal};
use hal::delay::CycleDelay;
use hal::gpio::{AnyPin, Input, Level, Output, OutputDrive, Pin, Pull};

use ch58x_hal::pac::Interrupt;
// use core::ptr::{read_volatile, write_volatile};

use embedded_hal_local::delay::DelayNs;
// use hal::delay::CycleDelay;


use hal::rtc::{Rtc};

use embassy_executor::Spawner;
use embassy_time::{Duration,  Timer};


// use flash::flash_test;

use ch58x_rs_kit::helpers::{enable_sleep, enter_sleep};
// use ch58x_rs_kit::audio::{get_char_for_t, chiptune_loop};
use ch58x_rs_kit::logger::init as init_logger;

use ch58x_rs_kit::{get_configured_rtc, log};

use hal::uart::UartTx;


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

#[embassy_executor::main(entry = "qingke_rt::entry")]
async fn main(spawner: Spawner) -> ! {
    let mut config = hal::Config::default(); 
    config.low_power = true; //800uA->150uA

    /* well, after writing sleep, something completely broke. I made the UART work in debug mode so it's
       possible, but I'm too much behind the schedule to fix this again.*/
    // if cfg!(debug_assertions) { 
    //     config.clock.use_pll_fuck_you(); //debug builds are unable to set any other clock than 6mhz
    // }
    // config.clock.enable_lse();
    let p = hal::init(config);
    hal::embassy::init();

    let mut ena = Output::new(p.PA4, Level::Low, OutputDrive::_5mA);
    ena.set_low();
    // ena.set_high();


    let but = Input::new(p.PB22, Pull::None);

    let serial = UartTx::new(p.UART0, p.PB7, Default::default()).unwrap();
    init_logger(serial);
    log!( "\n\n\nHello World!");

    let mut rtc = get_configured_rtc();

    // let pwm_out = Output::new(p.PA9, Level::Low,OutputDrive::_20mA);
    // chiptune_loop();

    if but.is_low() {
        log!("Button pressed, loopin' time\n");
        spawner.spawn(async_blink(p.PA8.degrade())).unwrap();

        // loop{
        //     but.wait_for_rising_edge().await;
        
        //     let now = rtc.now();
        //     log!("T{:02}:{:02}:{:02} \n", 
        //         now.hour, now.minute, now.second);
        // }
        // let led = Output::new(p.PA8, Level::Low, OutputDrive::_5mA);
        // blinky(led);

        loop{
            Timer::after(Duration::from_millis(1000)).await;
        }
    } else {
        let mut led = Output::new(p.PA8, Level::Low, OutputDrive::_5mA);
        led.set_high();

        rtc.enable_timing(hal::rtc::TimingMode::_0_5S);
        rtc.ack_timing();

        unsafe {
            qingke::pfic::enable_interrupt(Interrupt::RTC as u8);
        }
        enable_sleep();
        rtc_loop(rtc, led);
    }
}


fn rtc_loop(rtc: Rtc, mut led: Output<'_, ch58x_hal::peripherals::PA8>) -> ! {
    let mut counter: u32 = 0;
    let mut delay = CycleDelay;

    loop{
        // led.toggle();

        enter_sleep();       
        delay.delay_us(1000);
        let now = rtc.now();
        log!("T{:02}:{:02}:{:02}, loop {}\n", 
            now.hour, now.minute, now.second, counter);
        counter += 1;
        if (now.second % 5) == 0 {
            led.toggle();
        }
        delay.delay_us(500);
    }
}
