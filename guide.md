# Rust Mouse Guide (Pico 2 W + Mac)

This guide shows how to build the mouse firmware in Rust and put it on your Raspberry Pi Pico 2 W. It is written in simple steps. These are the steps I used on your Mac mini.

---

## What this project does

Your Pico 2 W becomes a USB mouse. You connect 6 push buttons to it:

| Button       | Pico pin (GPIO) | Physical pin |
|--------------|-----------------|--------------|
| Left click   | GPIO 10         | 14           |
| Right click  | GPIO 11         | 15           |
| Middle click | GPIO 12         | 16           |
| Scroll up    | GPIO 13         | 17           |
| Scroll down  | GPIO 14         | 19           |
| Speed (DPI)  | GPIO 15         | 20           |

**Wiring:** one leg of the button goes to the GPIO pin. The other leg goes to any `GND` pin. You don't need any resistor.

**Optional LED:** put an LED and a 330 ohm resistor between GPIO 22 (pin 29) and GND. It blinks 1, 2 or 3 times when you press the Speed button.

> The Pico 2 **W** has its built-in LED on the wireless chip, not on GPIO 25. So I moved the LED to GPIO 22.

**Movement sensor (PMW3360 breakout board):**

| Sensor pin | Pico pin (GPIO) | Physical pin |
|------------|-----------------|--------------|
| MISO       | GPIO 16         | 21           |
| CS / NCS   | GPIO 17         | 22           |
| SCK        | GPIO 18         | 24           |
| MOSI       | GPIO 19         | 25           |
| MOTION     | GPIO 20         | 26           |
| VCC        | 3V3 OUT         | 36           |
| GND        | GND             | 38 (or any GND) |

Put a 100nF and a 10µF capacitor between VCC and GND, close to the sensor. Power the sensor from **3V3 OUT**, never from VBUS (5V).

The full drawing is in `schematic.png` (also `schematic.svg`). The old picture `schematic_old_wrong.jpg` had wrong sensor wiring, so don't use it.

**Sensor firmware file (SROM):** the PMW3360 needs a small firmware file loaded into it every time it powers on. PixArt does not let me ship this file. Get `PMW3360_srom_0x04.bin` (about 4 KB) from the PMW3360 Arduino library, and copy it to `srom/pmw3360_srom.bin` in this folder. Then build again. If the file is empty, the mouse still works for clicks and scrolling, but tracking may be poor or not work.

If the cursor moves in the wrong direction, open `src/main.rs` and change `INVERT_X` or `INVERT_Y` to `true`.

---

## Step 1: Install the tools (one time only)

Open Terminal and run:

```bash
rustup target add thumbv8m.main-none-eabihf
brew install picotool
```

- The first line lets Rust build code for the Pico 2 chip (RP2350).
- The second line installs `picotool`, the tool that copies the firmware onto the Pico.

## Step 2: Go to the project folder

```bash
cd "/Users/aby/Desktop/Github/Rust Mouse"
```

## Step 3: Build the firmware

```bash
cargo build --release
```

The first build takes about a minute. When it finishes, the program is here:
`target/thumbv8m.main-none-eabihf/release/pico2-pro-mouse`

## Step 4: Put the Pico in BOOTSEL (update) mode

Pick one way.

**Way A, with the button (always works):**
1. Unplug the Pico.
2. Hold the white `BOOTSEL` button.
3. Plug in the USB cable while holding it.
4. Let go. A drive named `RP2350` shows up.

**Way B, with no button (only works while the Pico still runs MicroPython):**
```bash
ls /dev/cu.usbmodem*
stty -f /dev/cu.usbmodemXXXX 115200 raw -echo
printf 'import machine\r\nmachine.bootloader()\r\n' > /dev/cu.usbmodemXXXX
```
Replace `XXXX` with the number you see from the `ls` command. This is what I did the first time. After the mouse firmware is on the board, this way no longer works, so use Way A.

## Step 5: Install the firmware into the Pico

```bash
picotool load -u -v -x -t elf target/thumbv8m.main-none-eabihf/release/pico2-pro-mouse
```

- `-u` means update, `-v` means verify, and `-x` means start the program after loading.
- You should see `OK` and `The device was rebooted to start the application.`

You can also run just `cargo run --release`. It does Steps 3 and 5 together.

**Drag and drop way:** make a UF2 file and copy it to the `RP2350` drive:
```bash
picotool uf2 convert target/thumbv8m.main-none-eabihf/release/pico2-pro-mouse -t elf firmware.uf2 --family rp2350-arm-s
```
A ready `firmware.uf2` is already in this folder.

## Step 6: Check that it works

```bash
ioreg -p IOUSB -l | grep "Pico 2 Pro"
```

If you see `Pico 2 Pro Mouse`, the Mac sees your new mouse. Short a button pin to GND (with a wire or a button) and you should see a click.

---

## What I changed to make it work

1. **Library versions:** the old `Cargo.toml` had versions that did not work together. I updated them (`embassy-rp 0.4`, `embassy-usb 0.4`, `embassy-executor 0.7`, `embassy-time 0.4`).
2. **LED pin:** moved from GPIO 25 to GPIO 22, because this is a Pico 2 **W**.
3. **Installed** `picotool` and the Rust target.
4. **Flashed** the firmware. Your Pico had MicroPython on it before, and the Rust mouse firmware has replaced it.
5. **Added the sensor driver** (`src/pmw3360.rs`). Before, the sensor code was empty and the cursor could never move. Now the firmware talks to a PMW3360 over SPI, reads movement, and sets the sensor's real resolution (800 / 1600 / 3200 CPI) when you press the DPI button. If no sensor is connected, the firmware notices and the mouse still works for clicks and scrolling.
6. **Drew a correct schematic** (`schematic.png`). The old one had the LED on the wrong pin, 3.3V shorted to ground on GPIO 16, and sensor pins that did not match the chip's SPI pins.

## Go back to MicroPython later

Hold `BOOTSEL`, plug in USB, and copy a MicroPython `.uf2` file for Pico 2 W onto the `RP2350` drive.

## Problems and fixes

| Problem | Fix |
|---------|-----|
| `No accessible RP-series devices` | The Pico is not in BOOTSEL mode. Repeat Step 4. |
| No drive or no device appears | Use a USB cable that carries data, not a charge-only cable. |
| Build error about versions | Run `cargo update` and then build again. |
| Mouse does not move the cursor | Check the sensor wiring against `schematic.png`, and make sure `srom/pmw3360_srom.bin` is not empty. |
| Cursor moves the wrong way | Set `INVERT_X` or `INVERT_Y` to `true` in `src/main.rs`. |

## Next steps

- Use the Pico 2 W's Bluetooth to make the mouse wireless.
- Add a lift-off check using the sensor's `SQUAL` value.

---

## Progress log (what we achieved, and how to repeat it)

### 2026-11-16: Firmware with sensor driver installed

**What we achieved**
- Built the Rust mouse firmware for the Pico 2 W and flashed it. The Mac sees "Pico 2 Pro Mouse".
- Added the PMW3360 sensor driver (`src/pmw3360.rs`) and DPI levels of 800, 1600 and 3200 CPI.
- Drew a correct schematic (`schematic.png`).
- Pushed the project to GitHub: https://github.com/abyshergill/Rusty_Mouse (marked "not complete yet").

**How to repeat: update the firmware (every time you change the code)**
1. Open Terminal and go to the folder:
   ```bash
   cd "/Users/aby/Desktop/Github/Rust Mouse"
   ```
2. Build:
   ```bash
   cargo build --release
   ```
3. Put the Pico in BOOTSEL mode. **Unplug** the USB cable, **hold** the white BOOTSEL button, **plug in** the cable while holding, wait 2 seconds, then let go. (Check that a drive named `RP2350` shows up, or that `picotool info` works.)
4. Flash:
   ```bash
   picotool load -u -v -x -t elf target/thumbv8m.main-none-eabihf/release/pico2-pro-mouse
   ```
   You should see `OK`.
5. **Let go of BOOTSEL.** If the button is still held, the board keeps showing as "RP2350 Boot". If it does, unplug and replug the cable once without touching the button.
6. Check:
   ```bash
   ioreg -p IOUSB -l | grep "Pico 2 Pro"
   ```

**Lesson learned:** BOOTSEL only works if you hold the button *while plugging in*. Pressing it while the board is already plugged in does nothing. The software reboot trick (Way B) only worked once, when the Pico still ran MicroPython.

**How to repeat: push to GitHub**
```bash
git add -A
git commit -m "Describe your change"
git push
```
(`target/`, `.DS_Store` and the old wrong schematic are ignored by `.gitignore`.)

### Still to do
- Wire the PMW3360 sensor as in `schematic.png` and test cursor movement.
- Copy `PMW3360_srom_0x04.bin` into `srom/pmw3360_srom.bin` and rebuild.
- Clean up the old parts of `README.md` (still mentions GPIO 25).
- Optional: Bluetooth (wireless) mouse.
