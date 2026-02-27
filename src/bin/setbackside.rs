use anyhow::{anyhow, Result};
use image::{EncodableLayout, GrayImage};
use std::path::Path;
use rust_it8951::{It8951, Mode};

fn main() -> Result<()> {
    let path = std::env::args().nth(1).ok_or_else(|| {
        anyhow!("Usage: setbackside <image>\n\nSupported formats: JPEG, PNG, BMP, WebP, TIFF")
    })?;

    if !Path::new(&path).exists() {
        return Err(anyhow!("File not found: {}", path));
    }

    eprintln!("Connecting to E-ink display...");
    let mut it8951 = It8951::connect()?;
    let sys = it8951.get_system_info().ok_or_else(|| anyhow!("Failed to get system info"))?;
    let w = sys.width;
    let h = sys.height;
    eprintln!("Connected: {}x{}", w, h);

    eprintln!("Loading image: {}", path);
    let img_raw = image::open(&path)?
        .resize_to_fill(w, h, image::imageops::FilterType::Lanczos3)
        .to_luma8()
        .as_bytes()
        .to_vec();

    let img = image::DynamicImage::from(
        GrayImage::from_raw(w, h, img_raw)
            .ok_or_else(|| anyhow!("Failed to prepare image buffer"))?
    );

    eprintln!("Sending image...");
    it8951.load_region(&img, 0, 0)?;
    it8951.display_region(0, 0, w, h, Mode::GC16)?;
    eprintln!("Done.");
    Ok(())
}
