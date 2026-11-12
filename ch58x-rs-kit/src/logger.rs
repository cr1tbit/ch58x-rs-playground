use ch58x_hal::peripherals;
use ch58x_hal::uart::UartTx;
use core::fmt;
use core::fmt::Write; // for write_str on UartTx
use core::cell::RefCell;
use critical_section::Mutex;

// Single global UART transmitter protected by critical section so it can be
// used from interrupts and main code without race conditions.

static SERIAL: Mutex<RefCell<Option<UartTx<'static, peripherals::UART0>>>> = Mutex::new(RefCell::new(None));

/// Initialize global logger with an already configured UART transmitter.
/// Can be called only once; subsequent calls replace the previous UART.
///
/// The lifetime parameter on `UartTx` is erased to 'static internally.
pub fn init<'a>(uart: UartTx<'a, peripherals::UART0>) {
    // SAFETY: UART peripheral & pin are unique and live for program lifetime after being moved here.
    let uart_static: UartTx<'static, _> = unsafe { core::mem::transmute(uart) };
    critical_section::with(|cs| {
        *SERIAL.borrow(cs).borrow_mut() = Some(uart_static);
    });
}

#[inline(always)]
pub fn log_args(args: fmt::Arguments) {
    critical_section::with(|cs| {
        let cell = SERIAL.borrow(cs);
        if let Ok(mut uart_ref) = cell.try_borrow_mut() {
            if let Some(uart) = uart_ref.as_mut() {
                let _ = uart.write_fmt(args);
                let _ = uart.write_str("\n");
            }
        }
    });
}

#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => {
        $crate::logger::log_args(core::format_args!($($arg)*));
    };
}
