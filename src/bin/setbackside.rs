use anyhow::{anyhow, Result};
use std::path::Path;
use thinkbook_eink::{Display, Mode};

fn main() -> Result<()> {
    let path = std::env::args().nth(1).ok_or_else(|| {
        anyhow!("Usage: setbackside <image>\n\nSupported formats: JPEG, PNG, BMP, WebP, TIFF, SVG")
    })?;

    if !Path::new(&path).exists() {
        return Err(anyhow!("File not found: {}", path));
    }

    eprintln!("Connecting to E-ink display...");
    let mut display = Display::connect()?;
    let info = display.info();
    eprintln!("Connected: {}x{}", info.width, info.height);

    eprintln!("Loading image: {}", path);
    let img = image::open(&path)
        .map_err(|e| anyhow!("Failed to open image '{}': {}", path, e))?;

    eprintln!("Clearing display...");
    display.clear()?;

    eprintln!("Sending image...");
    display.show(&img, Mode::GC16)?;

    eprintln!("Done.");
    Ok(())
}
