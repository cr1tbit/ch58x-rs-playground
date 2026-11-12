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
- **Interrupts that "stay off" after an attach are this artefact.** Code can keep a constant in a
  register across a loop. embassy's `arch-riscv32` executor kept `8` in `s1` and re-enabled
  interrupts with `csrs 0x800, s1`. Once the attach overwrote `s1`, interrupts never came back on.
- **`mstatus.MIE = 0` in a halted sample is often normal.** Critical sections and interrupt
  handlers run with MIE = 0. `hal::embassy::Executor` sleeps with MIE = 1.

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

- **Use `hal::embassy::Executor`, not embassy-executor's `arch-riscv32`.**
  - qingke-rt 0.5 turns `wfi` into WFE. The `arch-riscv32` executor sleeps with interrupts
    masked, and a masked WFE only wakes on a *new* pending interrupt. One that became pending
    just before is lost, so USB enumeration stalls (0/6 on a CH582F) and BLE can stall too.
  - The HAL executor sleeps with interrupts enabled, and its `__pender` sets
    `PFIC_SCTLR.SETEVENT`. QingKe has a Cortex-M-like event register, so no wake-up is lost.
  - Entry point:
    `#[qingke_rt::entry] fn main() -> ! { hal::embassy::Executor::new().run(|s| s.must_spawn(app_main(s))) }`.
  - Don't enable `arch-*` features on `embassy-executor` anywhere in the build: they define a
    second `__pender`, and linking fails.
- **The USB interrupt fires on every SOF and NAKed token**, even when only reset, suspend and
  transfer interrupts are enabled: about 1000/s on an idle bus, about 23000/s while the host polls
  an IN endpoint. It wakes the CPU each time; the handler finds no flag.
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
