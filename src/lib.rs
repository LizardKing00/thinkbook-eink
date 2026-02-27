//! # thinkbook-eink
//!
//! Linux driver library for the Lenovo ThinkBook Plus Gen 1 E-ink lid display.
//!
//! The display is driven by an ITE IT8951 controller (USB ID 048d:8951),
//! communicating via SCSI-over-USB bulk transfers.
//!
//! ## Quick start
//!
//! ```no_run
//! use thinkbook_eink::{Display, Mode};
//!
//! let mut display = Display::connect().expect("Could not connect to E-ink display");
//! let info = display.info();
//! println!("Display resolution: {}x{}", info.width, info.height);
//!
//! let img = image::open("photo.png").unwrap();
//! display.clear().unwrap();
//! display.show(&img, Mode::GC16).unwrap();
//! ```

mod usb;

use anyhow::{anyhow, Result};
use image::{DynamicImage, EncodableLayout, GrayImage};
use rusb::{Context, UsbContext};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::mem;
use std::str;
use std::time::Duration;

const ENDPOINT_IN: u8 = 0x81;
const ENDPOINT_OUT: u8 = 0x02;
const MAX_TRANSFER: usize = 60 * 1024;

const VENDOR_ID: u16 = 0x048d;
const PRODUCT_ID: u16 = 0x8951;

const INQUIRY_CMD: [u8; 16] = [0x12, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
const GET_SYS_CMD: [u8; 16] = [
    0xfe, 0, 0x38, 0x39, 0x35, 0x31, 0x80, 0, 0x01, 0, 0x02, 0, 0, 0, 0, 0,
];
const LD_IMAGE_AREA_CMD: [u8; 16] = [
    0xfe, 0x00, 0x00, 0x00, 0x00, 0x00, 0xa2, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];
const DPY_AREA_CMD: [u8; 16] = [
    0xfe, 0x00, 0x00, 0x00, 0x00, 0x00, 0x94, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];
const PMIC_CONTROL: [u8; 16] = [
    0xfe, 0x00, 0x00, 0x00, 0x00, 0x00, 0xa3, 0x00,
    0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00,
];

/// Display refresh mode.
///
/// Slower modes produce better image quality. For static images, `GC16` is recommended.
/// For frequent updates (e.g. a clock), `DU` or `A2` reduce flicker.
#[repr(u32)]
#[derive(Serialize, Deserialize, PartialEq, Debug, Clone, Copy)]
pub enum Mode {
    /// Full blank/flash. Use this to clear the screen before displaying a new image.
    Init = 0,
    /// Fast update, black and white only. Good for text with frequent refreshes.
    DU,
    /// Full greyscale refresh (16 levels). Best quality, recommended for photos.
    GC16,
    /// Greyscale with ghosting reduction.
    GL16,
    GLR16,
    GLD16,
    /// 4-grey fast update.
    DU4,
    /// 2-bit (very fast). Use for animations or very frequent updates.
    A2,
}

/// Firmware and hardware information returned by the display controller.
#[derive(Debug, Clone)]
pub struct DisplayInfo {
    pub width: u32,
    pub height: u32,
    pub firmware_version: u32,
    pub vendor: String,
    pub product: String,
    pub revision: String,
}

impl fmt::Display for DisplayInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Vendor:    {}\nProduct:   {}\nRevision:  {}\nResolution: {}x{}\nFirmware:  v{}",
            self.vendor.trim(),
            self.product.trim(),
            self.revision.trim(),
            self.width,
            self.height,
            self.firmware_version,
        )
    }
}

#[repr(C)]
#[derive(Serialize, Deserialize, PartialEq, Debug)]
struct InquiryResult {
    padding: [u8; 8],
    vendor: [u8; 8],
    product: [u8; 16],
    revision: [u8; 4],
}

#[repr(C)]
#[derive(Serialize, Deserialize, PartialEq, Debug)]
struct SystemInfo {
    standard_cmd_no: u32,
    extended_cmd_no: u32,
    signature: u32,
    version: u32,
    width: u32,
    height: u32,
    update_buf_base: u32,
    image_buffer_base: u32,
    temperature_no: u32,
    mode: u32,
    frame_count: [u32; 8],
    num_img_buf: u32,
    reserved: [u32; 9],
}

#[repr(C)]
#[derive(Serialize, Deserialize, PartialEq, Debug)]
struct LoadImageAreaInfo {
    address: u32,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

#[repr(C)]
#[derive(Serialize, Deserialize, PartialEq, Debug)]
struct DisplayAreaInfo {
    address: u32,
    mode: u32,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    wait_ready: u32,
}

/// A connection to the ThinkBook Plus E-ink lid display.
pub struct Display {
    connection: usb::ScsiOverUsbConnection,
    system_info: SystemInfo,
    display_info: DisplayInfo,
}

impl Display {
    /// Connect to the E-ink display.
    ///
    /// Returns an error if the display is not found or cannot be claimed.
    /// Run with sudo or install the udev rule (see README) to avoid permission errors.
    pub fn connect() -> Result<Self> {
        let timeout = Duration::from_secs(30);
        let context = Context::new()?;

        let mut device_handle = context
            .open_device_with_vid_pid(VENDOR_ID, PRODUCT_ID)
            .ok_or_else(|| {
                anyhow!(
                    "E-ink display not found (USB {:04x}:{:04x}). \
                     Is the ThinkBook lid attached? Try running with sudo or install the udev rule.",
                    VENDOR_ID, PRODUCT_ID
                )
            })?;

        let _ = device_handle.set_auto_detach_kernel_driver(true);
        device_handle.claim_interface(0)?;

        let mut connection = usb::ScsiOverUsbConnection {
            device_handle,
            endpoint_out: ENDPOINT_OUT,
            endpoint_in: ENDPOINT_IN,
            timeout,
        };

        // Query firmware info
        let inquiry: InquiryResult = connection
            .read_command(&INQUIRY_CMD, bincode::options().with_little_endian())?;

        let vendor = str::from_utf8(&inquiry.vendor).unwrap_or("").trim().to_string();
        let product = str::from_utf8(&inquiry.product).unwrap_or("").trim().to_string();
        let revision = str::from_utf8(&inquiry.revision).unwrap_or("").trim().to_string();

        // Query system info
        let system_info: SystemInfo = connection
            .read_command(&GET_SYS_CMD, bincode::options().with_big_endian())?;

        let display_info = DisplayInfo {
            width: system_info.width,
            height: system_info.height,
            firmware_version: system_info.version,
            vendor,
            product,
            revision,
        };

        Ok(Self {
            connection,
            system_info,
            display_info,
        })
    }

    /// Returns display hardware and firmware information.
    pub fn info(&self) -> &DisplayInfo {
        &self.display_info
    }

    /// Clears the display with a full blank flash.
    pub fn clear(&mut self) -> Result<()> {
        let w = self.system_info.width;
        let h = self.system_info.height;
        self.display_region(0, 0, w, h, Mode::Init)
    }

    /// Display an image on the E-ink panel.
    ///
    /// The image will be automatically converted to 8-bit greyscale and
    /// resized/cropped to fit the display resolution (1920x1080).
    ///
    /// For best quality use `Mode::GC16`. For faster updates use `Mode::DU`.
    pub fn show(&mut self, img: &DynamicImage, mode: Mode) -> Result<()> {
        let w = self.system_info.width;
        let h = self.system_info.height;

        let resized = img
            .resize_to_fill(w, h, image::imageops::FilterType::Lanczos3)
            .to_luma8();

        let prepared = DynamicImage::from(
            GrayImage::from_raw(w, h, resized.as_bytes().to_vec())
                .ok_or_else(|| anyhow!("Failed to prepare image buffer"))?,
        );

        self.load_region(&prepared, 0, 0)?;
        self.display_region(0, 0, w, h, mode)?;
        Ok(())
    }

    fn load_region(&mut self, img: &DynamicImage, x: u32, y: u32) -> Result<()> {
        let address = self.system_info.image_buffer_base;
        let (width, height) = (img.width(), img.height());
        let info = LoadImageAreaInfo { address, x, y, width, height };
        let raw = img.to_luma8();
        let bytes = raw.as_bytes();

        for chunk in bytes.chunks(MAX_TRANSFER) {
            self.connection.write_command(
                &LD_IMAGE_AREA_CMD,
                info,
                chunk,
                bincode::options().with_little_endian(),
            )?;
        }
        Ok(())
    }

    fn display_region(&mut self, x: u32, y: u32, width: u32, height: u32, mode: Mode) -> Result<()> {
        let address = self.system_info.image_buffer_base;
        let info = DisplayAreaInfo {
            address,
            mode: mode as u32,
            x,
            y,
            width,
            height,
            wait_ready: 1,
        };
        self.connection.write_command(
            &DPY_AREA_CMD,
            info,
            &[],
            bincode::options().with_little_endian(),
        )?;
        Ok(())
    }

    /// Power the display on or off.
    pub fn set_power(&mut self, on: bool) -> Result<()> {
        let mut cmd = PMIC_CONTROL;
        cmd[10] = 1;
        cmd[11] = if on { 1 } else { 0 };
        self.connection.write_command_no_data(&cmd)?;
        Ok(())
    }
}
