use anyhow::Result;
use chrono::{Local, Timelike};
use image::{DynamicImage, GrayImage, Luma};
use imageproc::drawing::{draw_text_mut, text_size};
use rusttype::{Font, Scale};
use std::thread;
use std::time::Duration;
use rust_it8951::{It8951, Mode};

const WIDTH: u32 = 1920;
const HEIGHT: u32 = 1080;

fn load_font() -> Font<'static> {
    let font_paths = [
        "/usr/share/fonts/truetype/ubuntu/Ubuntu-B.ttf",
        "/usr/share/fonts/truetype/ubuntu/Ubuntu-R.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    ];
    for path in &font_paths {
        if let Ok(data) = std::fs::read(path) {
            if let Some(font) = Font::try_from_vec(data) {
                return font;
            }
        }
    }
    panic!("No usable font found. Install fonts-ubuntu or fonts-dejavu.");
}

fn render_clock(font: &Font) -> DynamicImage {
    let now = Local::now();
    let time_str = now.format("%H:%M").to_string();
    let date_str = now.format("%A, %d %B %Y").to_string();

    let mut img = GrayImage::from_pixel(WIDTH, HEIGHT, Luma([255u8]));

    let time_scale = Scale::uniform(300.0);
    let (tw, _) = text_size(time_scale, font, &time_str);
    let time_x = ((WIDTH as i32) - tw) / 2;
    let time_y = (HEIGHT as i32) / 2 - 200;
    draw_text_mut(&mut img, Luma([0u8]), time_x, time_y, time_scale, font, &time_str);

    let date_scale = Scale::uniform(80.0);
    let (dw, _) = text_size(date_scale, font, &date_str);
    let date_x = ((WIDTH as i32) - dw) / 2;
    let date_y = time_y + 320;
    draw_text_mut(&mut img, Luma([0u8]), date_x, date_y, date_scale, font, &date_str);

    DynamicImage::ImageLuma8(img)
}

fn main() -> Result<()> {
    eprintln!("Connecting to E-ink display...");
    let mut it8951 = It8951::connect()?;
    let sys = it8951.get_system_info().ok_or(anyhow::anyhow!("Failed to get system info"))?;
    let (w, h) = (sys.width, sys.height);
    eprintln!("Connected. Starting clock (Ctrl+C to stop).");

    let font = load_font();

    loop {
        let img = render_clock(&font);
        it8951.load_region(&img, 0, 0)?;
        it8951.display_region(0, 0, w, h, Mode::DU)?;

        let secs_remaining = 60 - Local::now().second();
        thread::sleep(Duration::from_secs(secs_remaining as u64));
    }
}
