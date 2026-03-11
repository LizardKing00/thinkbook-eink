use serde::Deserialize;
use anyhow::{anyhow, Result};
use image::{EncodableLayout, GrayImage, imageops};
use std::fs;
use std::path::Path;
use rust_it8951::{It8951, Mode};

const CONFIG_PATH: &str = "/etc/thinkbook-eink/server.toml";

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

#[derive(Deserialize, Clone, Debug, Default)]
struct Config {
    flip: Option<bool>,
    // Other fields are silently ignored by serde.
}

impl Config {
    fn load() -> Self {
        if Path::new(CONFIG_PATH).exists() {
            if let Ok(contents) = fs::read_to_string(CONFIG_PATH) {
                if let Ok(config) = toml::from_str(&contents) {
                    return config;
                }
            }
        }
        Config::default()
    }

    fn is_flipped(&self) -> bool {
        self.flip.unwrap_or(false)
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() -> Result<()> {
    let config = Config::load();

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
    eprintln!("Config: flip={}", config.is_flipped());

    eprintln!("Loading image: {}", path);
    let img_raw = image::open(&path)?
        .resize_to_fill(w, h, image::imageops::FilterType::Lanczos3)
        .to_luma8()
        .as_bytes()
        .to_vec();

    let gray = GrayImage::from_raw(w, h, img_raw)
        .ok_or_else(|| anyhow!("Failed to prepare image buffer"))?;

    let gray = if config.is_flipped() {
        imageops::rotate180(&gray)
    } else {
        gray
    };

    let img = image::DynamicImage::from(gray);

    eprintln!("Sending image...");
    it8951.load_region(&img, 0, 0)?;
    it8951.display_region(0, 0, w, h, Mode::GC16)?;
    eprintln!("Done.");
    Ok(())
}
