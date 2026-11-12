# ch58x-rs-playground

Rust on WCH CH58x (CH582/CH583), built on [ch58x-hal](https://github.com/cr1tbit/ch58x-hal)
(a fork with fixes for qingke-rt 0.5 / embassy 0.7).

- `ch58x-rs-kit`: shared library: UART logger, BLE peripheral services (NUS, current time,
  device info), settings storage in DataFlash, shift-register LED driver, fatal-error handler
- `ch58x-rs-kit/examples`: proof-of-concept firmware (BLE HID keyboard, BLE scan/advertise
  tests, atomics litmus test, external interrupt test, audio player, ...). These are
  experiments and not all of them are cleaned up.

Run an example with `cargo run --release --example <name>`.

Firmware using the kit lives next to its hardware:
[cyberlamp](https://github.com/cr1tbit/cyberlamp),
[artsywatch](https://github.com/rough-rat/artsywatch),
[papajbadge](https://github.com/rough-rat/papajbadge).
Depend on the kit with:

```toml
ch58x-rs-kit = { git = "https://github.com/cr1tbit/ch58x-rs-playground.git", branch = "main" }
```

`openocd/` is a pre-compiled openocd taken from MounRiver's release (no license).

This repository started as the papajbadge firmware (rough-rat/papajbadge-rs); its history is kept.

## BLE keyboard PoC on a second CH582 board

`ble_hid_keyboard` is an example in `ch58x-rs-kit`. It advertises as
`CH582-HID-PoC` using the chip's factory Bluetooth address. The active-low BOOT
button on **PB22** sends one `A` key tap (normally the character `a`) per press.
Holding the button does not repeat. Release BOOT after flashing; a button held
at startup must be released before it can type. Change the pin in
`ch58x-rs-kit/examples/ble_hid_keyboard.rs` if your board uses different wiring.

Build the USB-flashable image from the repository root:

```sh
sh analysis/build_ble_hid_keyboard.sh   # local tool, not in the repository
```

On the **second board**, unplug USB, hold BOOT, plug USB back in, then release
BOOT. Flash manually from the repository root:

```sh
wchisp flash target/riscv32imc-unknown-none-elf/release/examples/ble_hid_keyboard.bin
```

Pair `CH582-HID-PoC` in the operating system's Bluetooth settings, open a text
editor, focus its input field, and press BOOT. Accept any host pairing dialog;
the PoC uses Just Works pairing, so there is no passcode to enter on the board.
Key reports require an encrypted connection and host notification subscription.
The firmware sends key-down followed by an all-keys-up report after 40 ms;
presses before the host is ready are discarded. The host keyboard layout and
Caps Lock setting determine the resulting character. USB is only used for ROM
flashing; the running firmware is a **Bluetooth** keyboard, not a USB keyboard.

The service provides Report and Boot keyboard protocols, host LED output writes,
HID Information/Report Map/Control Point, Device Information with a development
PnP identity, and a fixed 100% Battery Level for this USB-powered PoC. Its service
layout follows [HID over GATT](https://www.bluetooth.com/specifications/specs/hid-over-gatt-profile-1-0/).
The current HAL does not persist bond keys to flash. After a power cycle or
reflash, forget/remove the device on the host and pair it again if reconnect fails.
The PoC also uses the raw polling executor.

The build and these host checks can run without either board connected:

```sh
rustc --edition 2021 --test --target x86_64-unknown-linux-gnu \
  ch58x-rs-kit/examples/ble_hid_keyboard/keyboard.rs -o /tmp/ble-hid-keyboard-tests
/tmp/ble-hid-keyboard-tests
```

These check boot-held-button suppression, debounce, one event per press, HID
report lengths, and fragmented Report Map reads. Actual pairing, notification
subscription and typed output require testing on the second board. The earlier
unused `ble_periph/hid_service.rs` draft is not used by this target.

## Requirements

You need WCH-LinkE probe - besides flashing/debugging via i-can't-belive-it's-not-SWD 
dual wire interface, you may also use 3V3 for power and RX for logs via UART.

Do not use the WCH-Link probe, it nearly works, but is too buggy to be useful.

## The workflow

### Setup

Requirements:

* Rustup - google "rustup" (or use your package manager). You may need to uninstall native rust package first.
* wlink - `cargo install wlink` (or use your package manager)

On Nix(OS), `nix-shell` to use `shell.nix`.

### Flashing as the WCH intended (via USB)

`wchisp flash <PATH>`. You must press "boot" button and reset power to enter ISP mode

### Enabling not-SWD interface

Supposedly this interface is not meant to be used for flashing. After unlocking
the device it works fine though. The workflow is much more convenient with debug
probe - with USB flashing, to load new firmware, one must disconnect and connect with "boot" button
pressed (hold reset -> hold boot -> release reset is not working either fo some 
reason)

The chip may need unlocking, using USB interface first.

Connect the device, and call `make mcu-unlock`

### flashing via not-SWD

`wlink -v flash <BINARY>`

example: `wlink flash target/riscv32imc-unknown-none-elf/release/examples/ble_adv_test`

You may need to unlock the chip first. 

### not-SWD + GDB workflow

In 1st shell, call `make spawn-openocd` - it must be running in the background.

It also claims the USB probe, so wlink will not work in parallel to it. 

You may need to kill openocd manually if something goes wrong.

-----

In 2nd shell, call either `make debug` or `make attach` (to skip loading new binary)

Refer to Makefile for more information.

### Flashing target put to sleep/wfi() mode

Reset the board with "boot" button pressed. This does not enter the bootloader
mode (like in esp32), but the firmware checks on start if the button is pressed
and enters blinky busyloop (so probe can attach to it with no issue).

### More info

Currently I'm using makefiles, but in the end, i'd like to keep the environment
rust-centric.

You may also customize yout workflow with .cargo/config.toml.


## Debug build issues (fixed)

Fixed in the ch58x-hal fork: `safe_modify` / `safe_write` do the protected store in a fixed
`asm!` sequence, so debug builds now set up clocks correctly. Original analysis:

This seems to be because `with_safe_access` is broken on debug builds. Its intent is to use some cursed hardware peripheral functionality where some registers are 'protected' and must be written in a very special dance alongside some other pokes to other MMIO, within some window of time. Debug builds create very blown up mutation lambdas that seem to blow over that cycle/time budget.

The effect is that some stuff that's guarded behind these pokes (eg. clock setup) does not get applied on debug builds:

```
debug build
0x40001000:	0x00000000	0x00000000	0x00140005	0x00200000 // 0x40001008 - clock register
0x40001010:	0x00000000	0x00000000	0x00000000	0x00000000 // PLL difference 0x48 vs 0x05
0x40001020:	0x092211df	0x00000200	0x00000000	0x82c31011 // conclusion - writing there doesnt work
0x40001030:	0x00000230	0x00000000	0x0083afab	0x00000000 // count register (irrelevant)
0x40001040:	0xcf0c8270	0x0010dd00	0x4a000094	0x00320000 // RB_SAFE_ACC_MODE 00 vs 11
0x40001050:	0x010000b2	0x00000000	0x0000a00f	0x00000000 // wdog (irrelevant)

--release
0x40001000:	0x00000000	0x00000000	0x00140048	0x00200000
0x40001010:	0x00000000	0x00000000	0x00000000	0x00000000
0x40001020:	0x092211df	0x00000200	0x00000000	0x82c31011
0x40001030:	0x00000230	0x00000000	0x01748622	0x00000000
0x40001040:	0xee0c8240	0x0010dd00	0xca000094	0x00320000
0x40001050:	0x010000b2	0x00000000	0x0000a00f	0x00000000
```
