#![feature(impl_trait_in_assoc_type)]
#![feature(type_alias_impl_trait)]
#![no_std]
#![no_main]

use ch58x_hal as hal;
use ch58x_hal::pac::{Interrupt, SYS};
use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
// use core::ptr::{read_volatile, write_volatile};
use embedded_hal_local::delay::DelayNs;
use hal::delay::CycleDelay;
use hal::gpio::{AnyPin, Input, Level, Output, OutputDrive, Pin, Pull};
// use hal::delay::CycleDelay;
use hal::rtc::Rtc;
use hal::uart::UartTx;
// use flash::flash_test;
use papajbadge_rs::helpers::{enable_sleep, enter_sleep};
// use papajbadge_rs::audio::{get_char_for_t, chiptune_loop};
use papajbadge_rs::logger::init as init_logger;
use papajbadge_rs::{get_configured_rtc, log};
// Import ABC parser from local crate
use abc_note_stream::AbcIter;


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

pub fn get_char_for_t(t: i32) -> u8 {
    let s = b"36364689";
    let part1 = ((t * (s[(t >> 13 & 7) as usize] & 15) as i32) / 12 & 128) as u8;
    let part2 = ((((((t >> 12) ^ ((t >> 12) - 2)) % 11) * t) / 4 | (t >> 13)) & 127) as u8;
    part1 + part2
}

pub fn chiptune_loop() -> ! {
    let mut period: u32 = 20;
    let mut delay = CycleDelay;

    unsafe {
        let tmr = setup_pwm();

        loop {
            let v = get_char_for_t(period as i32);
            tmr.fifo().write(|w| w.fifo().bits(v as u32));
            delay.delay_us(10);
            period = period.wrapping_add(1);
        }
    }
}

fn setup_pwm() -> ch58x_hal::pac::TMR0 {
    unsafe {
        let tmr = ch58x_hal::pac::TMR0::steal();
        tmr.ctrl_mod().write(|w| w.tmr_all_clear().bit(true));
        tmr.cnt_end().write(|w| w.cnt_end().bits(0x100));
        tmr.ctrl_mod().write(|w| w.tmr_all_clear().bit(true));
        tmr.ctrl_mod().write(|w| {
            w.tmr_all_clear()
                .bit(false)
                .tmr_mode_in()
                .bit(false)
                .tmr_count_en()
                .bit(true)
                .tmr_out_polar__tmr_cap_count()
                .bit(true)
                .tmr_pwm_repeat__tmr_cap_edge()
                .bits(0b11)
                .tmr_out_en()
                .bit(true)
        });
        tmr.fifo().write(|w| w.fifo().bits(0x08));
        tmr
    }
}

fn set_pwm(tmr: ch58x_hal::pac::TMR0, freq: Option<u32>, duty: u32) -> ch58x_hal::pac::TMR0 {
    {
        unsafe {
            if let Some(freq) = freq {
                let cnt_end = 24_000_000 / freq;
                let duty = (cnt_end * duty) / 100;

                log!("Freq: {}, cnt_end: {}, duty: {}", freq, cnt_end, duty);
                tmr.cnt_end().write(|w| w.cnt_end().bits(cnt_end));
                tmr.fifo().write(|w| w.fifo().bits(duty));
            } else {
                tmr.fifo().write(|w| w.fifo().bits(0x08));
            }
            tmr 
        }
    }
}

#[embassy_executor::main(entry = "qingke_rt::entry")]
async fn main(spawner: Spawner) -> ! {
    let mut config = hal::Config::default();
    config.low_power = true; //800uA->150uA

    let p = hal::init(config);
    hal::embassy::init();

    let mut ena = Output::new(p.PA4, Level::Low, OutputDrive::_5mA);
    // ena.set_low();
    ena.set_high();

    let but = Input::new(p.PB22, Pull::None);

    let serial = UartTx::new(p.UART0, p.PB7, Default::default()).unwrap();
    init_logger(serial);
    log!("\n\n\nHello World!");

    // print reset reason from reset_flag

    let sys = unsafe { &*SYS::PTR };
    let reason = sys.reset_status__glob_rom_cfg().read();

    log!("Reset reason: {:02x}", reason.bits());
    Timer::after(Duration::from_millis(100)).await;

    if but.is_low() {
        log!("Button pressed, loopin' time\n");
        spawner.spawn(async_blink(p.PA8.degrade())).unwrap();

        loop {
            Timer::after(Duration::from_millis(1000)).await;
        }
    } else {
        let _ = Output::new(p.PA9, Level::Low, OutputDrive::_20mA);
        // chiptune_loop();
        spawner.spawn(play_abc()).unwrap();
        loop{
            Timer::after(Duration::from_millis(1000)).await;
        }
    }
}

// Remove local Event/AbcIter definitions; use the versions from the `abc` crate
use ch58x_hal::peripherals::PA8;
#[embassy_executor::task]
async fn play_abc() {
    // let abc = b"
    //     X:1
    //     T:Barka
    //     M:6/8
    //     L:1/8
    //     R:jig
    //     K:C
    //     C6 | A3 A3- | AAB cBA | G3 G3- | G3 F2 E | F3 F3- | FFG AGF | E3 E3- |
    //     E3 C2 C | A3 A3- | AAB cBA | G3 G3- | G3 F2 E | F3 F3- | FDE FED | C6 | x6 |
    //     E6- | EDE FED | C3 C3- | C3 D2 E | F3 F3-  | F2 F FFE | D3 D3- | D2 G, C2 D |
    //     E3 E3- | E2 E F2 D | C3 C3 | x6 |
    //     C6 | A3 A3- | AAB cBA | G3 G3- | G3 F2 E | F3 F3- | FFG AGF | E3 E3- |
    //     E3 C2 C | A3 A3- | AAB cBA | G3 G3- | G3 F2 E | F3 F3- | FDE FED | C6 | x6 |
    //     ";
    let nokia = b"
        X:1
        T:nokia
        M:4/4
        L:1/8
        R:jig
        K:C
        C3 x C x C x | G7 | C2 G6 x |
        ";
    let iter = AbcIter::new(nokia, 320).expect("valid header");
    // let iter = AbcIter::new(abc, 320).expect("valid header");

    let mut tmr = setup_pwm();
    let ledpin = unsafe { PA8::steal() };
    let mut led = Output::new(ledpin, Level::Low, OutputDrive::_5mA);

    const scaler: u32 = 1000_00;

    for ev in iter {
        log!("{:?}", ev);
        let delay = ev.duration*60;
        // let delay = ev.duration*300;
        // let period_us :Option<u32> = match ev.freq {
        //     Some(f) => Some(scaler / f),
        //     None => None,
        // };
        // tmr = set_pwm(tmr, period_us, 50);
        tmr = set_pwm(tmr, ev.freq, 10);
        Timer::after(Duration::from_millis(delay as u64)).await;
        led.toggle();
    }
    led.set_low();

    // while let Some(event) = iter.next() {
    //     tmr = set_pwm(tmr, event.freq, 50);
    //     Timer::after(Duration::from_millis(event.duration as u64)).await;
    // }
}
