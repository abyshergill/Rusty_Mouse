//! Minimal PixArt PMW3360 driver (SPI mode 3, max 2 MHz).
//!
//! Wiring (SPI0):  MISO=GPIO16  CS=GPIO17  SCK=GPIO18  MOSI=GPIO19  MOTION=GPIO20
//!
//! The sensor needs its SROM firmware blob uploaded after every power-up.
//! Drop the 4094-byte file from PixArt / the Arduino PMW3360 library into
//! `srom/pmw3360_srom.bin`. If that file is empty the upload is skipped and
//! the sensor runs with reduced tracking quality (or not at all).

use defmt::{info, warn};
use embassy_rp::gpio::Output;
use embassy_rp::peripherals::SPI0;
use embassy_rp::spi::{Blocking, Spi};
use embassy_time::{block_for, Duration, Timer};

// Register map
const REG_PRODUCT_ID: u8 = 0x00;
const REG_MOTION: u8 = 0x02;
const REG_DELTA_Y_H: u8 = 0x06;
const REG_CONFIG1: u8 = 0x0F; // CPI = value * 100
const REG_CONFIG2: u8 = 0x10;
const REG_SROM_ENABLE: u8 = 0x13;
const REG_SROM_ID: u8 = 0x2A;
const REG_POWER_UP_RESET: u8 = 0x3A;
const REG_MOTION_BURST: u8 = 0x50;
const REG_SROM_LOAD_BURST: u8 = 0x62;

const PRODUCT_ID: u8 = 0x42;

static SROM: &[u8] = include_bytes!("../srom/pmw3360_srom.bin");

pub struct Pmw3360 {
    spi: Spi<'static, SPI0, Blocking>,
    cs: Output<'static>,
    present: bool,
    burst_armed: bool,
}

impl Pmw3360 {
    pub fn new(spi: Spi<'static, SPI0, Blocking>, cs: Output<'static>) -> Self {
        Self {
            spi,
            cs,
            present: false,
            burst_armed: false,
        }
    }

    pub fn is_present(&self) -> bool {
        self.present
    }

    fn write_reg(&mut self, reg: u8, value: u8) {
        self.cs.set_low();
        let _ = self.spi.blocking_write(&[reg | 0x80, value]);
        self.cs.set_high();
        block_for(Duration::from_micros(120)); // tSWW / tSWR
        self.burst_armed = false;
    }

    fn read_reg(&mut self, reg: u8) -> u8 {
        self.cs.set_low();
        let _ = self.spi.blocking_write(&[reg & 0x7F]);
        block_for(Duration::from_micros(160)); // tSRAD
        let mut buf = [0u8];
        let _ = self.spi.blocking_read(&mut buf);
        self.cs.set_high();
        block_for(Duration::from_micros(20)); // tSRW / tSRR
        self.burst_armed = false;
        buf[0]
    }

    /// Power-up sequence from the datasheet. Returns true if the sensor answered.
    pub async fn init(&mut self, cpi: u16) -> bool {
        self.cs.set_high();
        block_for(Duration::from_micros(10));
        self.cs.set_low();
        block_for(Duration::from_micros(10));
        self.cs.set_high();

        self.write_reg(REG_POWER_UP_RESET, 0x5A);
        Timer::after_millis(50).await;

        // Clear motion registers
        for reg in REG_MOTION..=REG_DELTA_Y_H {
            self.read_reg(reg);
        }

        let id = self.read_reg(REG_PRODUCT_ID);
        if id != PRODUCT_ID {
            warn!("PMW3360 not found (product id 0x{:02x}); cursor movement disabled", id);
            self.present = false;
            return false;
        }

        self.upload_srom().await;
        self.set_cpi(cpi);
        self.present = true;
        info!("PMW3360 ready, CPI {}", cpi);
        true
    }

    async fn upload_srom(&mut self) {
        if SROM.is_empty() {
            warn!("srom/pmw3360_srom.bin is empty: skipping SROM upload");
            return;
        }
        self.write_reg(REG_CONFIG2, 0x00); // disable rest mode during upload
        self.write_reg(REG_SROM_ENABLE, 0x1D);
        Timer::after_millis(10).await;
        self.write_reg(REG_SROM_ENABLE, 0x18);

        self.cs.set_low();
        let _ = self.spi.blocking_write(&[REG_SROM_LOAD_BURST | 0x80]);
        block_for(Duration::from_micros(15));
        for b in SROM {
            let _ = self.spi.blocking_write(&[*b]);
            block_for(Duration::from_micros(15));
        }
        self.cs.set_high();
        block_for(Duration::from_micros(200));

        let srom_id = self.read_reg(REG_SROM_ID);
        info!("PMW3360 SROM id 0x{:02x}", srom_id);
        self.write_reg(REG_CONFIG2, 0x00);
    }

    /// Set resolution in counts per inch (100..=12000, in steps of 100).
    pub fn set_cpi(&mut self, cpi: u16) {
        let v = (cpi.clamp(100, 12000) / 100) as u8;
        self.write_reg(REG_CONFIG1, v);
    }

    /// Read motion deltas using Motion Burst. Returns (dx, dy) in counts.
    pub fn read_motion(&mut self) -> (i16, i16) {
        if !self.present {
            return (0, 0);
        }
        if !self.burst_armed {
            self.write_reg(REG_MOTION_BURST, 0x00);
            self.burst_armed = true;
        }

        self.cs.set_low();
        let _ = self.spi.blocking_write(&[REG_MOTION_BURST]);
        block_for(Duration::from_micros(35)); // tSRAD_MOTBR
        let mut buf = [0u8; 6]; // Motion, Observation, XL, XH, YL, YH
        let _ = self.spi.blocking_read(&mut buf);
        self.cs.set_high();
        block_for(Duration::from_micros(1));

        if buf[0] & 0x80 == 0 {
            return (0, 0); // no motion flag
        }
        let dx = i16::from_le_bytes([buf[2], buf[3]]);
        let dy = i16::from_le_bytes([buf[4], buf[5]]);
        (dx, dy)
    }
}
