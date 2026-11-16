# Rusty_Mouse

> **Status: work in progress - this project is NOT complete yet.**
> Done: USB HID mouse firmware (buttons, scroll, DPI), PMW3360 SPI driver (untested on real hardware), corrected schematic.
> Still to do: test the sensor, add the PMW3360 SROM file (`srom/pmw3360_srom.bin`), wireless/Bluetooth, and update this README (parts below are from an older version and still mention GPIO 25 for the LED; the LED is now on GPIO 22 for the Pico 2 W).
> For current setup steps see [guide.md](guide.md) and [schematic.png](schematic.png).

# Pico 2 Pro Mouse Firmware & Hardware Guide

Custom 1000 Hz USB HID Mouse firmware written in asynchronous embedded Rust (`embassy-rp` + `embassy-usb`) targeting the Raspberry Pi Pico 2 (`RP2350` Cortex-M33 microcontroller).

## Project Overview

This project turns a Raspberry Pi Pico 2 into a low-latency USB 2.0 Full-Speed HID Mouse operating at a 1 ms (1000 Hz) polling interval. It uses internal GPIO pull-up resistors for zero-external-component tactile switch wiring, non-blocking asynchronous tasks for USB and LED status signaling, software switch debouncing, rate-controlled scrolling, and dynamic DPI multiplier switching.

Key capabilities include:

* True 1000 Hz deterministic polling loop driven by `embassy_time::Ticker`
* Non-blocking multi-tasking architecture (`hid_loop`, `usb_task`, and `led_task`)
* 10 ms per-switch hardware contact bounce filtering
* Typematic scroll controller (single tick on tap, smooth auto-repeat when held)
* Saturating 16-bit intermediate DPI scaling (`1x`, `2x`, `4x`) preventing 8-bit cursor reversal overflow
* Idle-suppressed USB HID reports that only transmit when button, wheel, or sensor state changes
* RP2350 BootROM `IMAGE_DEF` block and `picotool` UF2 packaging compatibility

## System Architecture

### Hardware Signal Path

Each tactile switch connects directly between an RP2350 GPIO pin (configured with internal `Pull::Up`) and any board `GND` pin. Pressing a switch pulls the pin to logic low (`0V`).

```text
+------------------------+       Internal Pull::Up       +----------------------------------+
| Tactile Switches (x6)  | ----------------------------> | RP2350 (Raspberry Pi Pico 2)     |
|  * Left   (GPIO 10)    |   Active-Low GPIO10..GPIO15   |                                  |
|  * Right  (GPIO 11)    |                               |  +----------------------------+  |
|  * Middle (GPIO 12)    |                               |  | Main Task (1000 Hz Ticker) |  |
|  * Scr Up (GPIO 13)    |       SPI / I2C (Future)      |  |  * DebouncedButton (10 ms) |  |
|  * Scr Dn (GPIO 14)    | <---------------------------> |  |  * ScrollController        |  |
|  * DPI    (GPIO 15)    |   Optical Sensor (PMW3360)    |  |  * Clamped DPI Scaling     |  |
+------------------------+                               |  +-------------+--------------+  |
                                                         |                |                 |
                                                         |     Signal<u8> |   HidWriter     |
                                                         |                v        |        |
+------------------------+            GPIO 25            |  +-------------+---+    |        |
| Onboard Status LED     | <---------------------------- |  | led_task        |    |        |
|  * 1/2/3 DPI Blinks    |                               |  +-----------------+    |        |
+------------------------+                               |                         v        |
                                                         |  +----------------------------+  |
+------------------------+       USB D+ / D- (1kHz)      |  | usb_task (USBCTRL_IRQ)     |  |
| Host PC (OS HID Stack) | <===========================> |  |  * 8-byte MouseReport      |  |
+------------------------+                               |  +----------------------------+  |
                                                         +----------------------------------+
```

### Firmware Task Architecture

The Embassy cooperative executor runs three concurrent async tasks on Cortex-M33 Thread Mode without RTOS heap overhead:

1. `main` (HID Input & Report Loop): Wakes every `1 ms` via `Ticker::every(Duration::from_millis(1))`. Samples all six GPIO inputs through `DebouncedButton`, updates `ScrollController`, checks for DPI profile changes, reads `(raw_x, raw_y)` from `read_optical_sensor()`, applies saturating `i16 -> i8` DPI scaling, and serializes an 8-byte `MouseReport` over USB when state changes occur.
2. `usb_task` (USB Device Stack): Owns the `'static` `UsbDevice` instance (backed by `StaticCell` descriptor buffers) and services USB bus enumeration, suspend/resume states, and `USBCTRL_IRQ` control transfers continuously in the background.
3. `led_task` (Status LED Controller): Waits asynchronously on `LED_BLINK_SIGNAL`. Whenever the user presses the DPI button, `main` signals the new profile index (`1`, `2`, or `3`) and immediately resumes 1000 Hz mouse polling while `led_task` blinks GPIO 25 in parallel (`100 ms` ON / `100 ms` OFF per blink).

### Repository File Layout

```text
pico2-pro-mouse/
├── .cargo/
│   └── config.toml     # Sets thumbv8m.main-none-eabihf target & picotool runner
├── src/
│   └── main.rs         # Embassy async firmware, USB HID, debouncer, DPI & LED tasks
├── build.rs            # Injects memory.x, link.x, and defmt.x linker scripts
├── Cargo.toml          # Crate manifest and RP2350 dependencies
├── memory.x            # RP2350 Flash (2048K) / SRAM (512K) layout & .start_block
└── README.md           # Hardware, architecture, flashing, and usage documentation
```

## Hardware Requirements & Pinout

### Bill of Materials

* Microcontroller: Raspberry Pi Pico 2 (`RP2350A`, non-wireless standard board with GPIO 25 onboard LED)
* Switches: 6x momentary normally-open (NO) tactile microswitches (e.g., Omron D2FC, Kailh GM 8.0, or breadboard tactile buttons)
* Cable: Data-capable Micro-USB cable
* Optional Sensor Expansion: SPI optical mouse sensor breakout (e.g., PixArt PMW3360, PMW3389, or PAW3395)

*Note on Pico 2 W:* On the wireless Pico 2 W variant, the onboard LED is routed through the CYW43439 wireless chip rather than GPIO 25. If using a Pico 2 W, connect an external LED + 330 ohm resistor to a spare GPIO pin (or use `cyw43` driver control).

### Wiring Table

Internal pull-up resistors (`Pull::Up`) are enabled in firmware, so external pull-up resistors are not required. Wire one terminal of each switch to its assigned GPIO pin and the other terminal to any Pico 2 `GND` pin (such as physical pins `13`, `18`, `23`, `28`, or `38`).

| Mouse Function | Pico 2 GPIO | Physical Board Pin | Switch Wiring | Firmware Behavior |
| :--- | :--- | :--- | :--- | :--- |
| Left Click | `GPIO 10` | Pin 14 | Switch terminal A -> Pin 14; terminal B -> `GND` | HID Button Bit 0 (`0x01`), 10 ms debounce |
| Right Click | `GPIO 11` | Pin 15 | Switch terminal A -> Pin 15; terminal B -> `GND` | HID Button Bit 1 (`0x02`), 10 ms debounce |
| Middle Click | `GPIO 12` | Pin 16 | Switch terminal A -> Pin 16; terminal B -> `GND` | HID Button Bit 2 (`0x04`), 10 ms debounce |
| Scroll Up | `GPIO 13` | Pin 17 | Switch terminal A -> Pin 17; terminal B -> `GND` | Wheel `+1` on press; repeats every 60 ms after 250 ms hold |
| Scroll Down | `GPIO 14` | Pin 19 | Switch terminal A -> Pin 19; terminal B -> `GND` | Wheel `-1` on press; repeats every 60 ms after 250 ms hold |
| DPI Cycle | `GPIO 15` | Pin 20 | Switch terminal A -> Pin 20; terminal B -> `GND` | Cycles `1x -> 2x -> 4x` on press edge |
| Status Light | `GPIO 25` | Onboard LED | Internal on Pico 2 (no external wiring needed) | Blinks `N` times (`1`, `2`, or `3`) on DPI change |

### Recommended SPI Optical Sensor Pinout (Optional Expansion)

When replacing the `read_optical_sensor()` placeholder with a PixArt SPI sensor (e.g., PMW3360 on `SPI0`), use `GPIO 16..19` so all button assignments remain untouched:

| Sensor Signal | Pico 2 GPIO | Physical Board Pin | Notes |
| :--- | :--- | :--- | :--- |
| `MISO` | `GPIO 16` | Pin 21 | `SPI0 RX` |
| `CS` (Chip Select) | `GPIO 17` | Pin 22 | Active-low manual `Output` |
| `SCK` | `GPIO 18` | Pin 24 | `SPI0 SCK` |
| `MOSI` | `GPIO 19` | Pin 25 | `SPI0 TX` |
| `MOTION` (Optional) | `GPIO 20` | Pin 26 | Active-low interrupt line |
| `VCC` / `VDD` | `3V3(OUT)` | Pin 36 | Check sensor board regulator voltage (`3.3V` logic!) |
| `GND` | `GND` | Pin 23 / 38 | Common ground |

## Prerequisites & Toolchain Setup

1. Install Rust via `rustup` (if not already installed) and add the ARMv8-M Mainline hardware-float target used by the RP2350 Cortex-M33 cores:

```bash
rustup target add thumbv8m.main-none-eabihf
```

2. Install `picotool` (v2.0 or newer, officially required for RP2350 UF2 family ID `0xe48bff59` and `IMAGE_DEF` verification):

* macOS (Homebrew): `brew install picotool`
* Ubuntu / Debian / Raspberry Pi OS: install from the Raspberry Pi SDK release or build from `https://github.com/raspberrypi/picotool`
* Windows: download prebuilt `picotool.exe` from the Raspberry Pi Pico SDK tools releases and add it to your `PATH`
* Alternative Rust tool: `cargo install elf2uf2-rs --locked` (ensure your installed version supports RP2350; otherwise prefer `picotool`)

## Building and Flashing the Firmware

### Step 1: Build the Release ELF

From inside the project directory (`pico2-pro-mouse/`), compile the optimized binary:

```bash
cargo build --release
```

This outputs the compiled ELF binary at:
`target/thumbv8m.main-none-eabihf/release/pico2-pro-mouse`

### Step 2: Put the Raspberry Pi Pico 2 into BOOTSEL Mode

1. Unplug the Pico 2 from your computer.
2. Press and hold down the white `BOOTSEL` button on the Pico 2 board.
3. While holding `BOOTSEL`, plug the Micro-USB cable into your computer.
4. Release `BOOTSEL`. A new USB Mass Storage drive named `RP2350` will appear on your computer.

### Step 3: Flash to the Board

#### Option A: Direct Flash via `cargo run` + `picotool` (Recommended)

Because `.cargo/config.toml` configures `picotool load -u -v -x -t elf` as the target runner, you can compile, verify, flash, and reboot in a single command while the Pico 2 is in BOOTSEL mode:

```bash
cargo run --release
```

#### Option B: Generate a `.uf2` File and Drag-and-Drop

Convert the compiled ELF into an RP2350 UF2 image (`--family rp2350-arm-s`):

```bash
picotool uf2 convert target/thumbv8m.main-none-eabihf/release/pico2-pro-mouse -t elf firmware.uf2 --family rp2350-arm-s
```

Copy `firmware.uf2` onto the mounted `RP2350` USB drive. The board will automatically unmount, reboot, blink the onboard green LED once, and enumerate on your host PC as `Pico 2 Pro Mouse` (`VID: 0xc0de`, `PID: 0xcafe`).

#### Option C: Debug Probe (`probe-rs` / Raspberry Pi Debug Probe)

If you have an SWD debug probe wired to the Pico 2 `SWDIO`/`SWCLK` pads, install `probe-rs-tools` and run with live `defmt` RTT logging:

```bash
probe-rs run --chip RP235x target/thumbv8m.main-none-eabihf/release/pico2-pro-mouse
```

## How to Use & Test

1. Enumeration Verification:
   * Linux: run `lsusb -d c0de:cafe` or `dmesg | tail -n 20` to confirm `Raspberry Pi Pico 2 Pro Mouse` attached as a `usbhid` input device.
   * Windows: check Device Manager under Mice and other pointing devices (`HID-compliant mouse`).
   * macOS: check `System Information -> USB` for `Pico 2 Pro Mouse`.
2. Startup Indicator: On power-up, the green onboard LED (`GPIO 25`) blinks `1` time to confirm DPI Level 1 (`1x`) is active.
3. Button Testing: Short `GPIO 10`, `GPIO 11`, or `GPIO 12` to `GND` via your wired switches to trigger Left, Right, and Middle mouse clicks cleanly without double-click bounce.
4. Scroll Wheel Emulation:
   * Tap `Scroll Up` (`GPIO 13`) or `Scroll Down` (`GPIO 14`) once for a single scroll notch.
   * Hold either button down for longer than `250 ms` to engage smooth continuous scrolling (`~16.6 ticks/sec`).
5. DPI Profile Cycling: Press the `DPI Cycle` button (`GPIO 15`) to step through sensitivity multipliers without interrupting active clicks or scrolling:
   * 1 blink = Level 1 (`1x` sensor delta)
   * 2 blinks = Level 2 (`2x` sensor delta)
   * 3 blinks = Level 3 (`4x` sensor delta)

## Key Firmware Improvements Over Initial Prototype

| Area | Original Prototype Issue | Production Firmware Fix |
| :--- | :--- | :--- |
| Compilation & Lifetimes | `Spawner::spawn` called as static fn; local stack buffers passed to `'static` `usb_task`; mismatched `Builder::new` signature | Uses `spawner.spawn(...)`, `StaticCell` `'static` buffers, and `embassy-usb 0.3` 4-buffer `Builder::new` |
| RP2350 Bootability | Missing crate `[package]`, `rp2350` invalid feature name, missing `IMAGE_DEF` & `memory.x` (board stays dead after flash) | Uses `rp235xa` feature, `#[link_section = ".start_block"] IMAGE_DEF`, `memory.x`, and `build.rs` |
| 1000 Hz Polling Stability | Blocking `Timer::after(100ms)` inside DPI branch froze cursor & clicks for up to `600 ms` | Offloaded LED blinking to an async `led_task` via `Signal`; replaced `Timer::after(1ms)` drift with `Ticker::every(1ms)` |
| Switch Reliability | Raw `is_low()` reads caused mechanical double-clicks; holding scroll blasted `1000 ticks/sec` | Added `10 ms` per-pin `DebouncedButton` and `ScrollController` (`250 ms` delay, `60 ms` repeat) |
| Cursor Math & USB Bus | `i8 * i8` panicked or wrapped negative on fast swipes (`40 * 4 = overflow`); flooded bus while idle | Widened scaling to `i16` with `.clamp(-128, 127)` and suppressed redundant idle USB HID reports |

## Troubleshooting

* Board reboots back into `RP2350` USB drive immediately after dropping `firmware.uf2`:
  Ensure `memory.x` and `build.rs` are present in the project root and that the UF2 was built with `--family rp2350-arm-s` (RP2040 UF2 family IDs are ignored by the RP2350 BootROM).
* Buttons appear stuck in the pressed state:
  Verify switches are wired to Normally Open (`NO`) terminals rather than Normally Closed (`NC`), and confirm they connect between `GPIO 10..15` and `GND` (never `3V3` or `VBUS`).
* No onboard LED blink on DPI press:
  Confirm you are using a standard Raspberry Pi Pico 2 (not Pico 2 W, where `GPIO 25` is not wired directly to the onboard LED).
