use serde::Deserialize;
use anyhow::Result;
use chrono::Local;
use image::{DynamicImage, GrayImage, Luma};
use imageproc::drawing::{draw_filled_rect_mut, draw_line_segment_mut, draw_text_mut, text_size};
use imageproc::rect::Rect;
use rusttype::{Font, Scale};
use std::collections::VecDeque;
use std::fs;
use std::path::Path;
use std::thread;
use std::time::Duration;
use sysinfo::{Components, Disks, Networks, System};
use rust_it8951::{It8951, Mode};

const W: u32 = 1920;
const H: u32 = 1080;

const BG:       Luma<u8> = Luma([18u8]);
const BRIGHT:   Luma<u8> = Luma([240u8]);
const MID:      Luma<u8> = Luma([160u8]);
const DIM:      Luma<u8> = Luma([80u8]);

const MARGIN: i32 = 40;
const CONFIG_PATH: &str = "/etc/thinkbook-eink/server.toml";

#[derive(Deserialize, Default)]
struct Config {
    nextcloud_url: Option<String>,
    nextcloud_user: Option<String>,
    nextcloud_password: Option<String>,
}

fn load_config() -> Config {
    if Path::new(CONFIG_PATH).exists() {
        if let Ok(contents) = fs::read_to_string(CONFIG_PATH) {
            if let Ok(config) = toml::from_str(&contents) {
                return config;
            }
        }
    }
    Config::default()
}

fn load_font(bold: bool) -> Font<'static> {
    let paths: &[&str] = if bold {
        &[
            "/usr/share/fonts/truetype/ubuntu/Ubuntu-B.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
        ]
    } else {
        &[
            "/usr/share/fonts/truetype/ubuntu/UbuntuMono-R.ttf",
            "/usr/share/fonts/truetype/ubuntu/Ubuntu-R.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        ]
    };
    for path in paths {
        if let Ok(data) = fs::read(path) {
            if let Some(font) = Font::try_from_vec(data) {
                return font;
            }
        }
    }
    panic!("No font found. Install fonts-ubuntu or fonts-dejavu.");
}

fn txt(img: &mut GrayImage, font: &Font, text: &str, x: i32, y: i32, size: f32, color: Luma<u8>) {
    draw_text_mut(img, color, x, y, Scale::uniform(size), font, text);
}

fn txt_c(img: &mut GrayImage, font: &Font, text: &str, cx: i32, y: i32, size: f32, color: Luma<u8>) {
    let (tw, _) = text_size(Scale::uniform(size), font, text);
    draw_text_mut(img, color, cx - tw / 2, y, Scale::uniform(size), font, text);
}

fn txt_r(img: &mut GrayImage, font: &Font, text: &str, rx: i32, y: i32, size: f32, color: Luma<u8>) {
    let (tw, _) = text_size(Scale::uniform(size), font, text);
    draw_text_mut(img, color, rx - tw, y, Scale::uniform(size), font, text);
}

fn hline(img: &mut GrayImage, x1: i32, x2: i32, y: i32, color: Luma<u8>) {
    draw_line_segment_mut(img, (x1 as f32, y as f32), (x2 as f32, y as f32), color);
}

fn vline(img: &mut GrayImage, x: i32, y1: i32, y2: i32, color: Luma<u8>) {
    draw_line_segment_mut(img, (x as f32, y1 as f32), (x as f32, y2 as f32), color);
}

fn corner_box(img: &mut GrayImage, x: i32, y: i32, w: i32, h: i32, arm: i32, color: Luma<u8>) {
    hline(img, x, x + arm, y, color);
    vline(img, x, y, y + arm, color);
    hline(img, x + w - arm, x + w, y, color);
    vline(img, x + w, y, y + arm, color);
    hline(img, x, x + arm, y + h, color);
    vline(img, x, y + h - arm, y + h, color);
    hline(img, x + w - arm, x + w, y + h, color);
    vline(img, x + w, y + h - arm, y + h, color);
}

fn dashed_hline(img: &mut GrayImage, x1: i32, x2: i32, y: i32, color: Luma<u8>) {
    let mut x = x1;
    while x < x2 {
        hline(img, x, (x + 12).min(x2), y, color);
        x += 18;
    }
}

fn seg_bar(img: &mut GrayImage, x: i32, y: i32, w: i32, h: i32, fraction: f32) {
    let segments = 20;
    let seg_w = (w - segments) / segments;
    let filled = (fraction.clamp(0.0, 1.0) * segments as f32) as i32;
    for i in 0..segments {
        let sx = x + i * (seg_w + 1);
        let color = if i < filled { BRIGHT } else { DIM };
        draw_filled_rect_mut(img, Rect::at(sx, y).of_size(seg_w as u32, h as u32), color);
    }
}

fn draw_graph(img: &mut GrayImage, x: i32, y: i32, w: i32, h: i32, values: &VecDeque<f64>) {
    if values.len() < 2 { return; }
    let max_val = values.iter().cloned().fold(0.0_f64, f64::max).max(1.0);
    let n = values.len();
    let points: Vec<(f32, f32)> = values.iter().enumerate().map(|(i, &v)| {
        let px = x as f32 + (i as f32 / (n - 1).max(1) as f32) * w as f32;
        let py = (y + h) as f32 - (v / max_val) as f32 * h as f32;
        (px, py)
    }).collect();
    for i in 1..points.len() {
        let (x0, y0) = points[i - 1];
        let col_x = x0 as i32;
        let col_top = y0 as i32;
        let col_bot = (y + h) as i32;
        if col_top < col_bot && col_x >= x && col_x < x + w {
            draw_filled_rect_mut(img, Rect::at(col_x, col_top).of_size(1, (col_bot - col_top) as u32), DIM);
        }
        draw_line_segment_mut(img, points[i - 1], points[i], BRIGHT);
    }
}

fn scanlines(img: &mut GrayImage) {
    let mut y = 0u32;
    while y < H {
        for x in 0..W {
            let v = img.get_pixel(x, y)[0].saturating_sub(6);
            img.put_pixel(x, y, Luma([v]));
        }
        y += 2;
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.1}GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    } else if bytes >= 1024 * 1024 {
        format!("{:.0}MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.0}KB", bytes as f64 / 1024.0)
    }
}

fn format_speed(bps: f64) -> String {
    if bps >= 1024.0 * 1024.0 {
        format!("{:.1} MB/S", bps / (1024.0 * 1024.0))
    } else if bps >= 1024.0 {
        format!("{:.0} KB/S", bps / 1024.0)
    } else {
        format!("{:.0} B/S", bps)
    }
}

fn format_uptime(secs: u64) -> String {
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;
    if days > 0 { format!("{}D {}H {}M", days, hours, mins) }
    else if hours > 0 { format!("{}H {}M", hours, mins) }
    else { format!("{}M", mins) }
}

fn check_nextcloud(config: &Config) -> (bool, u32) {
    let base = config.nextcloud_url.clone().unwrap_or_else(|| "https://localhost".to_string());
    let user = config.nextcloud_user.clone().unwrap_or_default();
    let pass = config.nextcloud_password.clone().unwrap_or_default();
    let client: reqwest::blocking::Client = match reqwest::blocking::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(Duration::from_secs(5))
        .build() {
        Ok(c) => c,
        Err(_) => return (false, 0),
    };
    let online = client.get(format!("{}/status.php", base))
        .send()
        .map(|r: reqwest::blocking::Response| r.status().is_success())
        .unwrap_or(false);
    if !online || user.is_empty() { return (online, 0); }
    let active_users = client
        .get(format!("{}/ocs/v2.php/apps/admin_audit/api/v1/activities", base))
        .basic_auth(&user, Some(&pass))
        .header("OCS-APIRequest", "true")
        .send()
        .ok()
        .and_then(|r: reqwest::blocking::Response| r.json::<serde_json::Value>().ok())
        .and_then(|j| j["ocs"]["data"]["activeUsers"]["last5minutes"].as_u64().map(|v| v as u32))
        .unwrap_or(0);
    (online, active_users)
}

fn render(fb: &Font, fr: &Font, sys: &System, upload_history: &VecDeque<f64>,
    download_history: &VecDeque<f64>, nc_online: bool, nc_users: u32, cpu_temp: f32) -> GrayImage {

    let mut img = GrayImage::from_pixel(W, H, BG);
    scanlines(&mut img);

    let now = Local::now();

    // Header
    txt(&mut img, fb, "SYS://NEXTCLOUD-NODE", MARGIN, 18, 48.0, BRIGHT);
    txt_r(&mut img, fb, &now.format("%H:%M").to_string(), W as i32 - MARGIN, 14, 64.0, BRIGHT);
    txt_r(&mut img, fr, &now.format("%d.%m.%Y").to_string(), W as i32 - MARGIN, 82, 32.0, MID);
    dashed_hline(&mut img, MARGIN, W as i32 - MARGIN, 110, MID);
    hline(&mut img, MARGIN, W as i32 - MARGIN, 112, DIM);

    // Status bar
    let status_str = if nc_online { "[ NEXTCLOUD: ONLINE ]" } else { "[ NEXTCLOUD: OFFLINE ]" };
    txt(&mut img, fb, status_str, MARGIN, 124, 34.0, if nc_online { BRIGHT } else { DIM });
    txt(&mut img, fr, &format!("ACTIVE USERS: {}", nc_users), 640, 130, 30.0, MID);
    txt(&mut img, fr, &format!("UPTIME: {}", format_uptime(System::uptime())), 1060, 130, 30.0, MID);
    txt_r(&mut img, fr, &format!("UPDATED: {}", now.format("%H:%M")), W as i32 - MARGIN, 130, 30.0, DIM);
    hline(&mut img, MARGIN, W as i32 - MARGIN, 172, DIM);

    // Row 1: stat boxes
    let col_w = (W as i32 - 2 * MARGIN - 3 * 20) / 4;
    let arm = 18;
    let r1y = 188;
    let r1h = 198;

    // RAM
    let ram_used = sys.used_memory();
    let ram_total = sys.total_memory();
    let ram_frac = ram_used as f32 / ram_total as f32;
    let bx = MARGIN;
    corner_box(&mut img, bx, r1y, col_w, r1h, arm, MID);
    txt(&mut img, fr, "// RAM", bx + arm + 6, r1y + 8, 24.0, DIM);
    txt_c(&mut img, fb, &format_bytes(ram_used), bx + col_w / 2, r1y + 40, 62.0, BRIGHT);
    txt_c(&mut img, fr, &format!("/ {}", format_bytes(ram_total)), bx + col_w / 2, r1y + 114, 28.0, MID);
    seg_bar(&mut img, bx + 16, r1y + 156, col_w - 32, 14, ram_frac);
    txt_c(&mut img, fr, &format!("{:.0}% USED", ram_frac * 100.0), bx + col_w / 2, r1y + 176, 22.0, DIM);

    // DISK
    let disks = Disks::new_with_refreshed_list();
    let (disk_used, disk_total) = disks.iter()
        .find(|d| d.mount_point().to_str() == Some("/"))
        .map(|d| (d.total_space() - d.available_space(), d.total_space()))
        .unwrap_or((0, 1));
    let disk_frac = disk_used as f32 / disk_total as f32;
    let bx = MARGIN + col_w + 20;
    corner_box(&mut img, bx, r1y, col_w, r1h, arm, MID);
    txt(&mut img, fr, "// DISK", bx + arm + 6, r1y + 8, 24.0, DIM);
    txt_c(&mut img, fb, &format_bytes(disk_used), bx + col_w / 2, r1y + 40, 62.0, BRIGHT);
    txt_c(&mut img, fr, &format!("/ {}", format_bytes(disk_total)), bx + col_w / 2, r1y + 114, 28.0, MID);
    seg_bar(&mut img, bx + 16, r1y + 156, col_w - 32, 14, disk_frac);
    txt_c(&mut img, fr, &format!("{:.0}% USED", disk_frac * 100.0), bx + col_w / 2, r1y + 176, 22.0, DIM);

    // CPU
    let cpu_usage = sys.global_cpu_info().cpu_usage();
    let bx = MARGIN + 2 * (col_w + 20);
    corner_box(&mut img, bx, r1y, col_w, r1h, arm, MID);
    txt(&mut img, fr, "// CPU", bx + arm + 6, r1y + 8, 24.0, DIM);
    txt_c(&mut img, fb, &format!("{:.0}%", cpu_usage), bx + col_w / 2, r1y + 40, 80.0, BRIGHT);
    seg_bar(&mut img, bx + 16, r1y + 156, col_w - 32, 14, cpu_usage / 100.0);
    txt_c(&mut img, fr, "LOAD", bx + col_w / 2, r1y + 176, 22.0, DIM);

    // TEMP
    let bx = MARGIN + 3 * (col_w + 20);
    corner_box(&mut img, bx, r1y, col_w, r1h, arm, MID);
    txt(&mut img, fr, "// TEMP", bx + arm + 6, r1y + 8, 24.0, DIM);
    txt_c(&mut img, fb, &format!("{:.0}", cpu_temp), bx + col_w / 2, r1y + 40, 80.0, BRIGHT);
    txt_c(&mut img, fr, "CELSIUS", bx + col_w / 2, r1y + 130, 28.0, MID);

    // Divider
    let r2y = r1y + r1h + 28;
    dashed_hline(&mut img, MARGIN, W as i32 - MARGIN, r2y, DIM);

    // Row 2: network graphs
    let r2y = r2y + 16;
    let graph_w = (W as i32 - 2 * MARGIN - 60) / 2;
    let graph_h = 180;

    let bx = MARGIN;
    txt(&mut img, fb, "// UPLOAD", bx, r2y, 30.0, MID);
    let cur_up = upload_history.back().cloned().unwrap_or(0.0);
    txt_r(&mut img, fb, &format!("TX: {}", format_speed(cur_up)), bx + graph_w, r2y + 2, 28.0, BRIGHT);
    corner_box(&mut img, bx, r2y + 38, graph_w, graph_h, arm, DIM);
    draw_graph(&mut img, bx + 4, r2y + 42, graph_w - 8, graph_h - 8, upload_history);

    let bx = MARGIN + graph_w + 60;
    txt(&mut img, fb, "// DOWNLOAD", bx, r2y, 30.0, MID);
    let cur_down = download_history.back().cloned().unwrap_or(0.0);
    txt_r(&mut img, fb, &format!("RX: {}", format_speed(cur_down)), bx + graph_w, r2y + 2, 28.0, BRIGHT);
    corner_box(&mut img, bx, r2y + 38, graph_w, graph_h, arm, DIM);
    draw_graph(&mut img, bx + 4, r2y + 42, graph_w - 8, graph_h - 8, download_history);

    // Footer
    let fy = H as i32 - 44;
    hline(&mut img, MARGIN, W as i32 - MARGIN, fy, DIM);
    dashed_hline(&mut img, MARGIN, W as i32 - MARGIN, fy + 2, DIM);
    txt(&mut img, fr, "THINKBOOK-EINK // GITHUB.COM/LIZARDKING00/THINKBOOK-EINK", MARGIN, fy + 10, 22.0, DIM);
    txt_r(&mut img, fr, "SYS:NOMINAL", W as i32 - MARGIN, fy + 10, 22.0, DIM);

    img
}

fn get_network_speeds(_sys: &System, prev_rx: u64, prev_tx: u64, elapsed_secs: f64) -> (f64, f64, u64, u64) {
    let mut total_rx: u64 = 0;
    let mut total_tx: u64 = 0;
    let networks = Networks::new_with_refreshed_list();
    for (_, data) in &networks {
        total_rx += data.total_received();
        total_tx += data.total_transmitted();
    }
    let rx_speed = if prev_rx > 0 && total_rx >= prev_rx { (total_rx - prev_rx) as f64 / elapsed_secs } else { 0.0 };
    let tx_speed = if prev_tx > 0 && total_tx >= prev_tx { (total_tx - prev_tx) as f64 / elapsed_secs } else { 0.0 };
    (rx_speed, tx_speed, total_rx, total_tx)
}

fn get_cpu_temp() -> f32 {
    let components = Components::new_with_refreshed_list();
    components.iter()
        .filter(|c| c.label().to_lowercase().contains("cpu") || c.label().to_lowercase().contains("core"))
        .map(|c| c.temperature())
        .fold(f32::NAN, f32::max)
        .max(0.0)
}

fn main() -> Result<()> {
    let config = load_config();
    eprintln!("Connecting to E-ink display...");
    let mut it8951 = It8951::connect()?;
    let sys_info = it8951.get_system_info().ok_or(anyhow::anyhow!("Failed to get system info"))?;
    let (dw, dh) = (sys_info.width, sys_info.height);
    eprintln!("Connected: {}x{}. Starting server dashboard (Ctrl+C to stop).", dw, dh);

    let font_bold = load_font(true);
    let font_reg = load_font(false);
    let mut upload_history: VecDeque<f64> = VecDeque::with_capacity(60);
    let mut download_history: VecDeque<f64> = VecDeque::with_capacity(60);
    let mut sys = System::new_all();
    let mut prev_rx: u64 = 0;
    let mut prev_tx: u64 = 0;
    let mut last_tick = std::time::Instant::now();

    loop {
        sys.refresh_all();
        let elapsed = last_tick.elapsed().as_secs_f64().max(1.0);
        last_tick = std::time::Instant::now();
        let (rx_speed, tx_speed, total_rx, total_tx) = get_network_speeds(&sys, prev_rx, prev_tx, elapsed);
        prev_rx = total_rx;
        prev_tx = total_tx;
        if upload_history.len() == 60 { upload_history.pop_front(); }
        if download_history.len() == 60 { download_history.pop_front(); }
        upload_history.push_back(tx_speed);
        download_history.push_back(rx_speed);
        let cpu_temp = get_cpu_temp();
        let (nc_online, nc_users) = check_nextcloud(&config);
        let img = render(&font_bold, &font_reg, &sys, &upload_history, &download_history, nc_online, nc_users, cpu_temp);
        let prepared = DynamicImage::ImageLuma8(img);
        it8951.load_region(&prepared, 0, 0)?;
        it8951.display_region(0, 0, dw, dh, Mode::GC16)?;
        eprintln!("[{}] RAM:{:.0}% CPU:{:.0}% TEMP:{:.0}C TX:{} RX:{} NC:{}",
            Local::now().format("%H:%M:%S"),
            sys.used_memory() as f32 / sys.total_memory() as f32 * 100.0,
            sys.global_cpu_info().cpu_usage(), cpu_temp,
            format_speed(tx_speed), format_speed(rx_speed),
            if nc_online { "ONLINE" } else { "OFFLINE" });
        thread::sleep(Duration::from_secs(60));
    }
}
