use anyhow::Result;
use image::{EncodableLayout, GrayImage};
use rust_it8951::{It8951, Mode};

fn main() -> Result<()> {
    eprintln!("Connecting to E-ink display (test_direct)...");
    let mut it8951 = It8951::connect()?;
    let sys = it8951
        .get_system_info()
        .ok_or_else(|| anyhow::anyhow!("Failed to get system info"))?;
    let w = sys.width;
    let h = sys.height;
    eprintln!("Connected: {}x{}", w, h);

    // Match the working eink-clock path as closely as possible:
    // no extra power/VCOM fiddling, no INIT clear, just load + DU.
    let img_raw = image::open("/tmp/ready.png")?
        .resize_to_fill(w, h, image::imageops::FilterType::Lanczos3)
        .to_luma8()
        .as_bytes()
        .to_vec();
    let img = image::DynamicImage::from(
        GrayImage::from_raw(w, h, img_raw)
            .ok_or_else(|| anyhow::anyhow!("Failed to prepare image buffer"))?
    );

    eprintln!("Loading image data...");
    it8951.load_region(&img, 0, 0)?;

    eprintln!("Triggering display refresh (DU)...");
    it8951.display_region(0, 0, w, h, Mode::DU)?;

    eprintln!("test_direct done.");
    Ok(())
}
