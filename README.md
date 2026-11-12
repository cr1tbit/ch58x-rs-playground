# papajbadge-rs

Rust firmware for CH582 boards, built on [ch58x-hal](https://github.com/cr1tbit/ch58x-hal)
(a fork with fixes for qingke-rt 0.5 / embassy 0.7).

| Crate | Contents |
| --- | --- |
| `ch58x-rs-kit` | Shared library: UART logger, BLE peripheral services (NUS, current time, device info), settings storage, shift-register LED driver, fatal-error handler. `examples/` holds the PoCs (BLE HID keyboard, BLE scan/advertise tests, atomics litmus test, ...) |
| `cyberlamp-rs` | Cyberlamp firmware (`cyberlamp_fw`) and its command protocol |
| `artsywatch-rs` | Artsywatch firmware (`artsywatch_fw`), uses `ledface_layout` |
| `papajbadge` | papajbadge firmware (`cebula_fw`), RTC sleep loop and tune playback |
| `ledface_layout` | LED watch face layout format |

Run firmware with `cargo run --release --bin <name>` and examples with
`cargo run --release --example <name>` from the repository root.

`openocd/` is a pre-compiled openocd taken from MounRiver's release (no license).

## Cyberlamp Bluetooth console

`cyberlamp_fw` exposes the [Nordic UART Service](https://nrfconnectdocs.nordicsemi.com/ncs/latest/nrf/libraries/bluetooth/services/nus.html)
for lamp commands. Connect to `21:37:04:20:69:96` with a BLE UART terminal or GATT
client, enable notifications on TX, and write text to RX.

| Attribute | UUID | Operation |
| --- | --- | --- |
| Service | `6E400001-B5A3-F393-E0A9-E50E24DCCA9E` | Discover |
| RX | `6E400002-B5A3-F393-E0A9-E50E24DCCA9E` | Write / Write Without Response |
| TX | `6E400003-B5A3-F393-E0A9-E50E24DCCA9E` | Notify |

End each command with a newline; CRLF also works. Send one command and wait for
its newline-terminated reply before sending the next.

| Command | Effect |
| --- | --- |
| `brightness 20` | Set the current mode's brightness step, `0..40` (0 is off) |
| `mix 20` | Set white colour mix, `0..40` (0 is cool, 40 is warm) |
| `mode white` | Select `white`, `uv`, or `ir`; each remembers its own brightness |
| `status` | Read mode, mix, current brightness, and all three per-mode brightness settings |
| `help` | List commands |

For example, writing `brightness 20\n` produces `OK brightness=20\n` on TX.
Settings use the same two-second delayed DataFlash save as physical knob/button
changes. `mix` configures the white mix even while another mode is selected.

The current ATT MTU is 23: split writes into at most 20 bytes and join TX
notifications until a newline arrives. Commands may span writes; a line may
contain up to 64 bytes before its terminator. Invalid commands return `ERR ...`
and leave settings unchanged. Disconnect clears partial lines and notification
subscriptions; subscribe again after reconnecting. If RX is overrun (ATT error
`0x11`), send a blank newline to resynchronize, then retry the command.

The pure parser checks can run on the host:

```sh
rustc --edition 2021 --test --target x86_64-unknown-linux-gnu \
  cyberlamp-rs/src/protocol.rs -o /tmp/cyberlamp-protocol-tests
/tmp/cyberlamp-protocol-tests
```

On Linux, after flashing the board, the hardware check discovers NUS, exercises
commands and notifications, and restores the initial settings. The optional power-cycle
check verifies persistence by toggling the WCH-Link 3.3 V output on a probe-powered
board, as in the Makefile's `toggle-3v3` target. Keep other BLE clients disconnected
and leave the physical controls untouched during this check.

```sh
python analysis/test_cyberlamp_nus.py --power-cycle   # local tool, not in the repository
```

Use UART and external BLE clients for debugging: debugger memory reads can hang
the running board.

For intermittent faults, run the UART monitor before reproducing the problem:

```sh
python -u analysis/monitor_cyberlamp_uart.py   # local tool, not in the repository
```

It uses the WCH-Link's stable USB device ID, reconnects after USB replug, and writes
timestamped rotating logs to `/tmp/cyberlamp-uart.log`. USB names such as
`/dev/ttyACM0` can change when another adapter is connected. A background instance's
PID is in `/tmp/cyberlamp-uart-monitor.pid`; stop that process before opening another
UART reader. Panic and CPU exception handling disable LED output and report the
fault over UART, then halt. They do not blink the LED output-enable pin.

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

### Building

Final firmware: `cargo build --release --bin cebula_fw`

Other firmware and examples are listed in the table at the top.

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

example: `wlink flash target/riscv32imc-unknown-none-elf/release/cebula_fw`

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


## Progress

- [x] environment
    - [x] extract required files from https://github.com/ch32-rs/ch58x-hal to this repo
    - [x] fix TIM0 SVD description
    - [x] prepare gdb environment
    - [x] Logs via UART
- [ ] board bringup
    - [x] check if bluetooth works at all
    - [x] run PoC PWM output
    - [ ] PWM + DMA for audio output (TIM0 DOES NOT WORK WITH DMA)
    - [x] speaker (works but requires manual rework, full power bootloops the device)
    - [ ] power
        - [ ] check power consumption + sleep modes
            - [x] run (6mA @ 6mhz)
            - [x] idle (1.6mA, agrees with datasheet)
            - [x] halt (150-250uA, should be 320uA)
            - [ ] sleep
            - [ ] BT listen
            - [ ] BT ADV
            - [ ] BT connection
        - [x] check CR2032/USB power XORing (removed in rev2)
        - [x] check if LDO stable (removed in rev2)
    - [x] clocks
        - [x] check LSE (waveform visible without activating in code, probing glitches time counting without halting MCU)
        - [x] check HSE (probing glitches UART baudrate)
        - [x] check RTC (set/read works, counting works)
    - [ ] onboard flash
        - [x] chip ID + read
        - [ ] write
        - [ ] DMA
    - [ ] SD card
    - [x] bluetooth
        - [x] run BLE connection (using examples from hal)
        - [x] run bluetooth scan/ADV reception 
- [ ] firmware
    - [x] integrate embassy async
    - [ ] UART not working in debug builds (clock not settable in debug other than 6mhz)
    - [x] UART logging
    - [ ] bluetooth features
        - [x] peripheral PoC
        - [x] set RTC from BLE (almost working, but RTC cannot be set from callback context(?))
        - [ ] badge-to-badge comm
        - [ ] listen to meme adv packets


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
