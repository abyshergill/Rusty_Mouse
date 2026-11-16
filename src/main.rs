#![no_std]
#![no_main]

use defmt::{info, warn};
use embassy_executor::Spawner;
use embassy_rp::bind_interrupts;
use embassy_rp::block::ImageDef;
use embassy_rp::gpio::{Input, Level, Output, Pull};
use embassy_rp::peripherals::USB;
use embassy_rp::spi::{self, Phase, Polarity, Spi};
use embassy_rp::usb::{Driver, InterruptHandler};
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Ticker, Timer};
use embassy_usb::class::hid::{Config as HidConfig, HidWriter, State};
use embassy_usb::{Builder, Config as UsbConfig, UsbDevice};
use static_cell::StaticCell;
use usbd_hid::descriptor::{MouseReport, SerializedDescriptor};
use {defmt_rtt as _, panic_probe as _};

mod pmw3360;
use pmw3360::Pmw3360;

/// Required by the RP2350 BootROM to identify a valid Secure ARM executable in Flash.
#[link_section = ".start_block"]
#[used]
pub static IMAGE_DEF: ImageDef = ImageDef::secure_exe();

bind_interrupts!(struct Irqs {
    USBCTRL_IRQ => InterruptHandler<USB>;
});

/// Static USB descriptor & state buffers required for `'static` background USB task.
static CONFIG_DESC: StaticCell<[u8; 256]> = StaticCell::new();
static BOS_DESC: StaticCell<[u8; 256]> = StaticCell::new();
static MSOS_DESC: StaticCell<[u8; 256]> = StaticCell::new();
static CONTROL_BUF: StaticCell<[u8; 64]> = StaticCell::new();
static HID_STATE: StaticCell<State> = StaticCell::new();

/// Async signal to trigger non-blocking LED blink patterns on DPI changes.
static LED_BLINK_SIGNAL: Signal<ThreadModeRawMutex, u8> = Signal::new();

const DEBOUNCE_TIME: Duration = Duration::from_millis(10);
const SCROLL_INITIAL_DELAY: Duration = Duration::from_millis(250);
const SCROLL_REPEAT_INTERVAL: Duration = Duration::from_millis(60);
/// Sensor resolution levels (counts per inch). Cycled by the DPI button.
const CPI_LEVELS: [u16; 3] = [800, 1600, 3200];
/// Flip these if the cursor moves the wrong way for how the sensor is mounted.
const INVERT_X: bool = false;
const INVERT_Y: bool = false;

/// Debouncer for active-low tactile switches (`Pull::Up` -> pressed == `is_low()`).
struct DebouncedButton<'d> {
    pin: Input<'d>,
    stable_pressed: bool,
    raw_last: bool,
    last_transition: Instant,
}

impl<'d> DebouncedButton<'d> {
    fn new(pin: Input<'d>) -> Self {
        let pressed = pin.is_low();
        Self {
            pin,
            stable_pressed: pressed,
            raw_last: pressed,
            last_transition: Instant::now(),
        }
    }

    /// Updates button state and returns `(is_pressed, just_pressed_edge)`.
    fn poll(&mut self, now: Instant) -> (bool, bool) {
        let raw = self.pin.is_low();
        if raw != self.raw_last {
            self.raw_last = raw;
            self.last_transition = now;
        }

        let mut just_pressed = false;
        if (now - self.last_transition) >= DEBOUNCE_TIME && raw != self.stable_pressed {
            self.stable_pressed = raw;
            if self.stable_pressed {
                just_pressed = true;
            }
        }

        (self.stable_pressed, just_pressed)
    }
}

/// Rate-limited tactile scroll handler (1 tick on press, auto-repeat when held).
struct ScrollController {
    direction: i8,
    next_tick_at: Instant,
}

impl ScrollController {
    const fn new() -> Self {
        Self {
            direction: 0,
            next_tick_at: Instant::MIN,
        }
    }

    fn update(&mut self, up_pressed: bool, down_pressed: bool, now: Instant) -> i8 {
        let desired_dir = match (up_pressed, down_pressed) {
            (true, false) => 1,
            (false, true) => -1,
            _ => 0,
        };

        if desired_dir == 0 {
            self.direction = 0;
            return 0;
        }

        if desired_dir != self.direction {
            self.direction = desired_dir;
            self.next_tick_at = now + SCROLL_INITIAL_DELAY;
            return desired_dir;
        }

        if now >= self.next_tick_at {
            self.next_tick_at = now + SCROLL_REPEAT_INTERVAL;
            return desired_dir;
        }

        0
    }
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());
    info!("Initializing Pico 2 Pro Mouse firmware...");

    let driver = Driver::new(p.USB, Irqs);

    let mut usb_config = UsbConfig::new(0xc0de, 0xcafe);
    usb_config.manufacturer = Some("Raspberry Pi");
    usb_config.product = Some("Pico 2 Pro Mouse");
    usb_config.serial_number = Some("1.0");
    usb_config.max_power = 100;
    usb_config.max_packet_size_0 = 64;

    let mut builder = Builder::new(
        driver,
        usb_config,
        CONFIG_DESC.init([0; 256]),
        BOS_DESC.init([0; 256]),
        MSOS_DESC.init([0; 256]),
        CONTROL_BUF.init([0; 64]),
    );

    let hid_config = HidConfig {
        report_descriptor: MouseReport::desc(),
        request_handler: None,
        poll_ms: 1, // 1000 Hz USB HID polling interval
        max_packet_size: 8,
    };

    let mut hid_writer = HidWriter::<_, 8>::new(
        &mut builder,
        HID_STATE.init(State::new()),
        hid_config,
    );

    let usb_dev = builder.build();
    let status_led = Output::new(p.PIN_22, Level::Low);

    // Spawn independent USB & LED background tasks so main 1000 Hz HID loop never stalls
    spawner.spawn(usb_task(usb_dev)).unwrap();
    spawner.spawn(led_task(status_led)).unwrap();

    // Configure GPIO inputs with internal pull-ups (switches wire GPIO -> GND)
    let mut btn_left = DebouncedButton::new(Input::new(p.PIN_10, Pull::Up));
    let mut btn_right = DebouncedButton::new(Input::new(p.PIN_11, Pull::Up));
    let mut btn_middle = DebouncedButton::new(Input::new(p.PIN_12, Pull::Up));
    let mut btn_scroll_up = DebouncedButton::new(Input::new(p.PIN_13, Pull::Up));
    let mut btn_scroll_down = DebouncedButton::new(Input::new(p.PIN_14, Pull::Up));
    let mut btn_dpi = DebouncedButton::new(Input::new(p.PIN_15, Pull::Up));

    // Optical sensor on SPI0: MISO=GPIO16, CS=GPIO17, SCK=GPIO18, MOSI=GPIO19, MOTION=GPIO20
    let mut spi_config = spi::Config::default();
    spi_config.frequency = 2_000_000;
    spi_config.phase = Phase::CaptureOnSecondTransition;
    spi_config.polarity = Polarity::IdleHigh;
    let spi = Spi::new_blocking(p.SPI0, p.PIN_18, p.PIN_19, p.PIN_16, spi_config);
    let sensor_cs = Output::new(p.PIN_17, Level::High);
    let motion_pin = Input::new(p.PIN_20, Pull::Up);
    let mut sensor = Pmw3360::new(spi, sensor_cs);

    let mut scroll_ctrl = ScrollController::new();
    let mut current_dpi_idx: usize = 0;
    let mut last_buttons: u8 = 0;

    sensor.init(CPI_LEVELS[current_dpi_idx]).await;

    // Indicate initial DPI profile (Level 1 = 1 blink)
    LED_BLINK_SIGNAL.signal(1);

    let mut ticker = Ticker::every(Duration::from_millis(1));

    loop {
        ticker.next().await;
        let now = Instant::now();

        // 1. Poll & debounce standard buttons
        let (left, _) = btn_left.poll(now);
        let (right, _) = btn_right.poll(now);
        let (middle, _) = btn_middle.poll(now);

        let mut buttons: u8 = 0;
        if left {
            buttons |= 1 << 0;
        }
        if right {
            buttons |= 1 << 1;
        }
        if middle {
            buttons |= 1 << 2;
        }

        // 2. Poll scroll buttons with repeat-rate limiting
        let (scroll_up, _) = btn_scroll_up.poll(now);
        let (scroll_down, _) = btn_scroll_down.poll(now);
        let wheel = scroll_ctrl.update(scroll_up, scroll_down, now);

        // 3. Poll DPI cycle button (edge-triggered, non-blocking LED notification)
        let (_, dpi_just_pressed) = btn_dpi.poll(now);
        if dpi_just_pressed {
            current_dpi_idx = (current_dpi_idx + 1) % CPI_LEVELS.len();
            let level = (current_dpi_idx + 1) as u8;
            let cpi = CPI_LEVELS[current_dpi_idx];
            info!("Switched to DPI level {} ({} CPI)", level, cpi);
            if sensor.is_present() {
                sensor.set_cpi(cpi);
            }
            LED_BLINK_SIGNAL.signal(level);
        }

        // 4. Read optical sensor deltas (only when MOTION line is asserted) and clamp to i8
        let (raw_x, raw_y) = if sensor.is_present() && motion_pin.is_low() {
            sensor.read_motion()
        } else {
            (0, 0)
        };
        let raw_x = if INVERT_X { raw_x.saturating_neg() } else { raw_x };
        let raw_y = if INVERT_Y { raw_y.saturating_neg() } else { raw_y };
        let final_x = raw_x.clamp(i8::MIN as i16, i8::MAX as i16) as i8;
        let final_y = raw_y.clamp(i8::MIN as i16, i8::MAX as i16) as i8;

        // 5. Only send a USB HID report when state changes or motion/scroll is non-zero
        let has_activity =
            buttons != last_buttons || final_x != 0 || final_y != 0 || wheel != 0;

        if has_activity {
            let report = MouseReport {
                buttons,
                x: final_x,
                y: final_y,
                wheel,
                pan: 0,
            };

            match hid_writer.write_serialize(&report).await {
                Ok(()) => {
                    last_buttons = buttons;
                }
                Err(e) => {
                    warn!("USB HID write skipped: {:?}", e);
                }
            }
        }
    }
}

#[embassy_executor::task]
async fn usb_task(mut usb_dev: UsbDevice<'static, Driver<'static, USB>>) -> ! {
    usb_dev.run().await
}

#[embassy_executor::task]
async fn led_task(mut status_led: Output<'static>) -> ! {
    loop {
        let blink_count = LED_BLINK_SIGNAL.wait().await;
        for _ in 0..blink_count {
            status_led.set_high();
            Timer::after(Duration::from_millis(100)).await;
            status_led.set_low();
            Timer::after(Duration::from_millis(100)).await;
        }
    }
}
