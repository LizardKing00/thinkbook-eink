//! # thinkbook-eink
//!
//! Linux driver and CLI tools for the Lenovo ThinkBook Plus Gen 1 E-ink lid display.

pub use rust_it8951::Mode;
use anyhow::{anyhow, Result};
use image::DynamicImage;
use rust_it8951::It8951;
use std::fmt;

/// Display hardware and firmware information.
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

/// A connection to the ThinkBook Plus E-ink lid display.
pub struct Display {
    inner: It8951,
    info: DisplayInfo,
}

impl Display {
    /// Connect to the E-ink display.
    pub fn connect() -> Result<Self> {
        let mut inner = It8951::connect()?;

        let inquiry = inner.inquiry()?;
        let sys = inner.get_system_info()
            .ok_or_else(|| anyhow!("Failed to get system info"))?;

        let info = DisplayInfo {
            width: sys.width,
            height: sys.height,
            firmware_version: sys.version,
            vendor: inquiry.vendor.trim().to_string(),
            product: inquiry.product.trim().to_string(),
            revision: inquiry.revision.trim().to_string(),
        };

        Ok(Self { inner, info })
    }

    /// Returns display hardware and firmware information.
    pub fn info(&self) -> &DisplayInfo {
        &self.info
    }

    /// Clears the display with a full blank flash.
    pub fn clear(&mut self) -> Result<()> {
        let w = self.info.width;
        let h = self.info.height;
        self.inner.display_region(0, 0, w, h, Mode::INIT)?;
        Ok(())
    }

    /// Display an image. Automatically resized and converted to greyscale.
    pub fn show(&mut self, img: &DynamicImage, mode: Mode) -> Result<()> {
        let w = self.info.width;
        let h = self.info.height;

        let prepared = img
            .resize_to_fill(w, h, image::imageops::FilterType::Lanczos3)
            .to_luma8();
        let prepared = DynamicImage::ImageLuma8(prepared);

        self.inner.load_region(&prepared, 0, 0)?;
        self.inner.display_region(0, 0, w, h, mode)?;
        Ok(())
    }

    /// Display a pre-prepared greyscale image (must be exactly the right dimensions).
    pub fn show_raw(&mut self, img: &DynamicImage, mode: Mode) -> Result<()> {
        let w = self.info.width;
        let h = self.info.height;
        self.inner.load_region(img, 0, 0)?;
        self.inner.display_region(0, 0, w, h, mode)?;
        Ok(())
    }
}
