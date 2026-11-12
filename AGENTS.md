# Notes for agents working on CH58x firmware

Lessons from debugging CH582 boards with a WCH-LinkE probe. They apply to this repository and to
firmware built on `ch58x-rs-kit` / [ch58x-hal](https://github.com/cr1tbit/ch58x-hal).

## The debug probe changes what it observes

- **Attaching to a running CH582 corrupts CPU registers.** Any attach (`openocd ... -c init`,
  `wlink regs`, `wlink status`, `wlink dump`) overwrites `a1` (`0x40001041`), `s1` (`0xffffff82`,
  the sign-extended chip ID) and `a3`. The firmware may misbehave from that moment on.
  - Register values or crashes seen *after* an attach are not evidence of a firmware bug.
  - Reset after every attach (`wlink reset`) before drawing conclusions about the running
    firmware.
  - Observe a running target over UART (or USB CDC), not through the probe.
- **Interrupts that "stay off" after an attach are this artefact.** The embassy executor keeps the
  constant `8` in `s1` and re-enables interrupts with `csrs 0x800, s1`. Once the attach overwrites
  `s1`, interrupts never come back on.
- **`mstatus.MIE = 0` in the executor loop is normal.** The executor masks interrupts while it
  polls tasks and while it sleeps in `wfi`, so a halted sample there shows MIE = 0.

## Reading memory

- **`wlink dump` cannot read RAM.** For addresses `0x2000xxxx` it returns the flash contents at
  the same offset. Use OpenOCD from this repository instead:

  ```sh
  cd openocd && ./openocd -f wch-riscv.cfg -c init -c halt -c "mdw 0x20000000 4" -c resume -c shutdown
  wlink reset   # the attach corrupted registers, see above
  ```

- **Trace buffer pattern** for events you can't log in real time:
  1. Add a `#[no_mangle] static mut TRACE: [u8; N]` plus a length counter and record events into it.
  2. Let the firmware run until the problem has happened.
  3. Halt and read the buffer with OpenOCD. The data was written before the attach, so it's valid.
  4. Find the address with `nm <elf> | grep TRACE`. It changes with every build.
  - Don't record high-rate events (e.g. USB SOF every 1 ms), or the buffer fills in a fraction
    of a second.
- **Hardware breakpoints on code that runs from RAM** (`#[highcode]`, interrupt handlers) did
  not trigger with WCH's OpenOCD.

## Known runtime issues

- **WFE idle can miss interrupts.** qingke-rt 0.5 turns `wfi` into `wfe`. Even with SEVONPEND
  (set in `hal::init()`), a pending USB interrupt does not always wake the CPU, so
  `#[embassy_executor::main]` can stall USB enumeration and BLE. Workaround: a raw executor
  polled continuously, see `cyberlamp_fw` or ch58x-hal's `examples/usb-cdc.rs`. The CPU then
  never sleeps.
- **LR/SC atomics are broken on CH582** (`sc.w` never fails); AMO instructions are fine. Build
  for `riscv32imc-unknown-none-elf` and do not enable qingke's `unsafe-trust-wch-atomics`.
- **Peripheral interrupt handlers** must use `#[qingke_rt::interrupt]`, otherwise they are not in
  the hardware vector table.

## Host side

- **Opening `/dev/ttyACM*` can block**, because it waits for a modem carrier. Open with
  `O_NONBLOCK` and set `CLOCAL`, or use `picocom`.
- **Put the port in raw mode before reading.** With the default terminal settings the host echoes
  received characters back to the device. A firmware that echoes its input then talks to itself
  in an endless loop.
- **BlueZ reconnects paired devices by address.** If firmware reuses an address the PC once
  paired (e.g. the chip's factory MAC from an earlier HID test), the PC connects automatically
  and takes the only peripheral connection slot. Remove the stale pairing with
  `bluetoothctl remove <addr>`.
