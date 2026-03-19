use serde::Deserialize;
use anyhow::Result;
use chrono::{Local, Timelike};
use image::{DynamicImage, GrayImage, Luma, imageops};
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

const MARGIN: i32 = 40;
const CONFIG_PATH: &str = "/etc/thinkbook-eink/server.toml";

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "lowercase")]
enum Theme {
    #[default]
    Dark,
    Light,
}

#[derive(Deserialize, Clone, Debug, Default)]
struct Config {
    flip: Option<bool>,
    theme: Option<Theme>,
    nextcloud_url: Option<String>,
    nextcloud_user: Option<String>,
    nextcloud_password: Option<String>,
    nextcloud_token: Option<String>,
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

    fn is_dark(&self) -> bool {
        matches!(self.theme.as_ref().unwrap_or(&Theme::Dark), Theme::Dark)
    }
}

struct Palette {
    bg:     Luma<u8>,
    bright: Luma<u8>,
    mid:    Luma<u8>,
    dim:    Luma<u8>,
}

impl Palette {
    fn from_config(config: &Config) -> Self {
        if config.is_dark() {
            Palette {
                bg:     Luma([10u8]),
                bright: Luma([245u8]),
                mid:    Luma([170u8]),
                dim:    Luma([90u8]),
            }
        } else {
            Palette {
                bg:     Luma([245u8]),
                bright: Luma([10u8]),
                mid:    Luma([80u8]),
                dim:    Luma([160u8]),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Fonts
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Drawing helpers
// ---------------------------------------------------------------------------

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

fn draw_graph(img: &mut GrayImage, x: i32, y: i32, w: i32, h: i32, values: &VecDeque<f64>, p: &Palette) {
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
            draw_filled_rect_mut(img, Rect::at(col_x, col_top).of_size(1, (col_bot - col_top) as u32), p.dim);
        }
        draw_line_segment_mut(img, points[i - 1], points[i], p.bright);
    }
}

fn scanlines(img: &mut GrayImage, config: &Config) {
    // In dark mode: slightly darken every other row to increase contrast.
    // In light mode: slightly lighten every other row for the equivalent effect.
    let mut y = 0u32;
    while y < H {
        for x in 0..W {
            let v = img.get_pixel(x, y)[0];
            let adjusted = if config.is_dark() {
                v.saturating_sub(10)
            } else {
                v.saturating_add(10)
            };
            img.put_pixel(x, y, Luma([adjusted]));
        }
        y += 2;
    }
}

// ---------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Nextcloud
// ---------------------------------------------------------------------------

fn nc_base_url(config: &Config) -> String {
    config
        .nextcloud_url
        .as_deref()
        .unwrap_or("https://localhost")
        .trim_end_matches('/')
        .to_string()
}

fn apply_nc_auth(req: reqwest::blocking::RequestBuilder, config: &Config) -> reqwest::blocking::RequestBuilder {
    if let Some(user) = config.nextcloud_user.as_deref() {
        if let Some(token) = config.nextcloud_token.as_deref() {
            return req.basic_auth(user, Some(token));
        }
        if let Some(pass) = config.nextcloud_password.as_deref() {
            return req.basic_auth(user, Some(pass));
        }
    }
    req
}

fn check_nextcloud(config: &Config) -> (bool, String, u32, String) {
    let base = nc_base_url(config);
    let client = match reqwest::blocking::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(Duration::from_secs(5))
        .build() {
        Ok(c) => c,
        Err(_) => return (false, String::new(), 0, base),
    };
    let start = std::time::Instant::now();
    let req = client.get(format!("{}/status.php", base));
    let status_resp = apply_nc_auth(req, config).send();
    let elapsed_ms = start.elapsed().as_millis() as u32;

    let (online, version) = match status_resp {
        Ok(resp) if resp.status().is_success() => {
            let ver = resp
                .json::<serde_json::Value>()
                .ok()
                .and_then(|j| j["versionstring"].as_str().map(|s| s.to_string()))
                .unwrap_or_default();
            (true, ver)
        }
        _ => (false, String::new()),
    };

    (online, version, elapsed_ms, base)
}

/// Fetch rich server info from the Nextcloud serverinfo API.
/// Returns (active_5m, active_1h, active_24h, app_updates, core_update_available).
fn fetch_serverinfo(config: &Config) -> (u64, u64, u64, u64, bool) {
    let defaults = (0, 0, 0, 0, false);
    let base = match config.nextcloud_url.as_deref() {
        Some(_) => nc_base_url(config),
        None => return defaults,
    };
    // Need either a serverinfo token (NC-Token header) or user+password/app-password
    let has_token = config.nextcloud_token.is_some();
    let has_basic = config.nextcloud_user.is_some()
        && (config.nextcloud_token.is_some() || config.nextcloud_password.is_some());
    if !has_token && !has_basic {
        return defaults;
    }
    let client = match reqwest::blocking::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(Duration::from_secs(5))
        .build() {
        Ok(c) => c,
        Err(_) => return defaults,
    };
    let url = format!("{}/ocs/v2.php/apps/serverinfo/api/v1/info?format=json", base);
    eprintln!("[serverinfo] fetching {}", url);
    let mut req = client.get(&url).header("OCS-APIRequest", "true");
    // Serverinfo supports its own token via NC-Token header (set via occ).
    // When using NC-Token, do NOT also send Basic Auth — it would conflict.
    if let Some(token) = config.nextcloud_token.as_deref() {
        req = req.header("NC-Token", token);
    } else {
        req = apply_nc_auth(req, config);
    }
    let resp = match req.send() {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => {
            eprintln!("[serverinfo] HTTP {}", r.status());
            return defaults;
        }
        Err(e) => {
            eprintln!("[serverinfo] request failed: {}", e);
            return defaults;
        }
    };
    let json: serde_json::Value = match resp.json() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[serverinfo] JSON parse error: {}", e);
            return defaults;
        }
    };
    let data = &json["ocs"]["data"];

    let active_5m = data["activeUsers"]["last5minutes"].as_u64().unwrap_or(0);
    let active_1h = data["activeUsers"]["last1hour"].as_u64().unwrap_or(0);
    let active_24h = data["activeUsers"]["last24hours"].as_u64().unwrap_or(0);

    let app_updates = data["nextcloud"]["system"]["apps"]["num_updates_available"]
        .as_u64()
        .unwrap_or(0);
    let core_update_val = &data["nextcloud"]["system"]["update"]["available"];
    let core_update = core_update_val.as_bool().unwrap_or(false)
        || core_update_val.as_str().map(|s| !s.is_empty()).unwrap_or(false);

    eprintln!(
        "[serverinfo] users={}/{}/{} app_updates={} core_update={}",
        active_5m, active_1h, active_24h, app_updates, core_update
    );

    (active_5m, active_1h, active_24h, app_updates, core_update)
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

fn render(
    fb: &Font,
    fr: &Font,
    sys: &System,
    upload_history: &VecDeque<f64>,
    download_history: &VecDeque<f64>,
    nc_online: bool,
    nc_version: &str,
    nc_latency_ms: u32,
    cpu_temp: f32,
    nc_url: &str,
    nc_active_5m: u64,
    nc_active_1h: u64,
    nc_active_24h: u64,
    nc_app_updates: u64,
    nc_core_update: bool,
    config: &Config,
) -> GrayImage {
    let p = Palette::from_config(config);
    let mut img = GrayImage::from_pixel(W, H, p.bg);
    scanlines(&mut img, config);

    let now = Local::now();

    // Header
    txt(&mut img, fb, "SYS://NEXTCLOUD-NODE", MARGIN, 18, 48.0, p.bright);
    txt_r(&mut img, fb, &now.format("%H:%M").to_string(), W as i32 - MARGIN, 14, 64.0, p.bright);
    txt_r(&mut img, fr, &now.format("%d.%m.%Y").to_string(), W as i32 - MARGIN, 82, 32.0, p.mid);
    dashed_hline(&mut img, MARGIN, W as i32 - MARGIN, 110, p.mid);
    hline(&mut img, MARGIN, W as i32 - MARGIN, 112, p.dim);

    // Status bar — row 1
    let status_str = if nc_online { "[ NEXTCLOUD: ONLINE ]" } else { "[ NEXTCLOUD: OFFLINE ]" };
    txt(&mut img, fb, status_str, MARGIN, 118, 30.0, if nc_online { p.bright } else { p.dim });
    let ver_label = if nc_online && !nc_version.is_empty() {
        format!("NC VER: {}", nc_version)
    } else {
        "NC VER: -".to_string()
    };
    let lat_label = if nc_online {
        format!("LATENCY: {}MS", nc_latency_ms)
    } else {
        "LATENCY: N/A".to_string()
    };
    txt(&mut img, fr, &ver_label, 580, 120, 28.0, p.mid);
    txt(&mut img, fr, &lat_label, 980, 120, 28.0, p.mid);
    txt_r(
        &mut img, fr,
        &format!("UPTIME: {}", format_uptime(System::uptime())),
        W as i32 - MARGIN, 120, 28.0, p.mid,
    );

    // Status bar — row 2
    let users_label = if nc_online && (nc_active_5m > 0 || nc_active_1h > 0 || nc_active_24h > 0) {
        format!("USERS: {}/{}/{} (5M/1H/24H)", nc_active_5m, nc_active_1h, nc_active_24h)
    } else {
        "USERS: -/-/- (5M/1H/24H)".to_string()
    };
    txt(&mut img, fr, &users_label, MARGIN, 152, 26.0, p.mid);
    if nc_app_updates > 0 {
        let alert = format!("\u{26A0} {} APP UPDATES PENDING", nc_app_updates);
        txt(&mut img, fb, &alert, 560, 150, 26.0, p.bright);
    }
    if nc_core_update {
        txt(&mut img, fb, "\u{26A0} CORE UPDATE PENDING", 1060, 150, 26.0, p.bright);
    }
    hline(&mut img, MARGIN, W as i32 - MARGIN, 182, p.dim);

    // Row 1: stat boxes
    let col_w = (W as i32 - 2 * MARGIN - 3 * 20) / 4;
    let arm = 18;
    let r1y = 198;
    let r1h = 198;

    // RAM
    let ram_used = sys.used_memory();
    let ram_total = sys.total_memory();
    let bx = MARGIN;
    corner_box(&mut img, bx, r1y, col_w, r1h, arm, p.mid);
    txt(&mut img, fr, "// RAM", bx + arm + 6, r1y + 8, 24.0, p.dim);
    txt_c(&mut img, fb, &format_bytes(ram_used), bx + col_w / 2, r1y + 40, 62.0, p.bright);
    txt_c(&mut img, fr, &format!("/ {}", format_bytes(ram_total)), bx + col_w / 2, r1y + 114, 28.0, p.mid);
    let ram_frac = ram_used as f32 / ram_total as f32;
    txt_c(&mut img, fr, &format!("{:.0}% USED", ram_frac * 100.0), bx + col_w / 2, r1y + 176, 22.0, p.dim);

    // DISK
    let disks = Disks::new_with_refreshed_list();
    let (disk_used, disk_total) = disks.iter()
        .find(|d| d.mount_point().to_str() == Some("/"))
        .map(|d| (d.total_space() - d.available_space(), d.total_space()))
        .unwrap_or((0, 1));
    let bx = MARGIN + col_w + 20;
    corner_box(&mut img, bx, r1y, col_w, r1h, arm, p.mid);
    txt(&mut img, fr, "// DISK", bx + arm + 6, r1y + 8, 24.0, p.dim);
    txt_c(&mut img, fb, &format_bytes(disk_used), bx + col_w / 2, r1y + 40, 62.0, p.bright);
    txt_c(&mut img, fr, &format!("/ {}", format_bytes(disk_total)), bx + col_w / 2, r1y + 114, 28.0, p.mid);
    let disk_frac = disk_used as f32 / disk_total as f32;
    txt_c(&mut img, fr, &format!("{:.0}% USED", disk_frac * 100.0), bx + col_w / 2, r1y + 176, 22.0, p.dim);

    // CPU
    let bx = MARGIN + 2 * (col_w + 20);
    corner_box(&mut img, bx, r1y, col_w, r1h, arm, p.mid);
    txt(&mut img, fr, "// CPU", bx + arm + 6, r1y + 8, 24.0, p.dim);
    let cpu_usage = sys.global_cpu_info().cpu_usage();
    txt_c(&mut img, fb, &format!("{:.0}%", cpu_usage), bx + col_w / 2, r1y + 40, 80.0, p.bright);
    txt_c(&mut img, fr, "LOAD", bx + col_w / 2, r1y + 176, 22.0, p.dim);

    // TEMP
    let bx = MARGIN + 3 * (col_w + 20);
    corner_box(&mut img, bx, r1y, col_w, r1h, arm, p.mid);
    txt(&mut img, fr, "// TEMP", bx + arm + 6, r1y + 8, 24.0, p.dim);
    txt_c(&mut img, fb, &format!("{:.0}°C", cpu_temp), bx + col_w / 2, r1y + 40, 80.0, p.bright);
    txt_c(&mut img, fr, "CPU TEMP", bx + col_w / 2, r1y + 130, 28.0, p.mid);

    // Divider
    let r2y = r1y + r1h + 28;
    dashed_hline(&mut img, MARGIN, W as i32 - MARGIN, r2y, p.dim);

    // Row 2: network graphs
    let r2y = r2y + 16;
    let graph_w = (W as i32 - 2 * MARGIN - 60) / 2;
    let graph_h = 180;

    let bx = MARGIN;
    txt(&mut img, fb, "// UPLOAD", bx, r2y, 30.0, p.mid);
    let cur_up = upload_history.back().cloned().unwrap_or(0.0);
    txt_r(&mut img, fb, &format!("TX: {}", format_speed(cur_up)), bx + graph_w, r2y + 2, 28.0, p.bright);
    corner_box(&mut img, bx, r2y + 38, graph_w, graph_h, arm, p.dim);
    draw_graph(&mut img, bx + 4, r2y + 42, graph_w - 8, graph_h - 8, upload_history, &p);
    txt(&mut img, fr, "SPEED", bx + 10, r2y + 46, 20.0, p.dim);
    txt_r(&mut img, fr, "TIME ->", bx + graph_w - 10, r2y + 38 + graph_h + 4, 20.0, p.dim);
    txt_c(&mut img, fr, "TX MB/S (LAST 60 MIN)", bx + graph_w / 2, r2y + 38 + graph_h + 26, 20.0, p.dim);

    let bx = MARGIN + graph_w + 60;
    txt(&mut img, fb, "// DOWNLOAD", bx, r2y, 30.0, p.mid);
    let cur_down = download_history.back().cloned().unwrap_or(0.0);
    txt_r(&mut img, fb, &format!("RX: {}", format_speed(cur_down)), bx + graph_w, r2y + 2, 28.0, p.bright);
    corner_box(&mut img, bx, r2y + 38, graph_w, graph_h, arm, p.dim);
    draw_graph(&mut img, bx + 4, r2y + 42, graph_w - 8, graph_h - 8, download_history, &p);
    txt(&mut img, fr, "SPEED", bx + 10, r2y + 46, 20.0, p.dim);
    txt_r(&mut img, fr, "TIME ->", bx + graph_w - 10, r2y + 38 + graph_h + 4, 20.0, p.dim);
    txt_c(&mut img, fr, "RX MB/S (LAST 60 MIN)", bx + graph_w / 2, r2y + 38 + graph_h + 26, 20.0, p.dim);

    // Nextcloud URL summary
    let summary_y = H as i32 - 96;
    let display_url = nc_url
        .strip_prefix("https://")
        .or_else(|| nc_url.strip_prefix("http://"))
        .unwrap_or(nc_url);
    txt(&mut img, fr, &format!("NEXTCLOUD URL: {}", display_url), MARGIN, summary_y, 22.0, p.dim);
    txt(&mut img, fr, "CFG: /etc/thinkbook-eink/server.toml", MARGIN, summary_y + 24, 20.0, p.dim);

    // Footer
    let fy = H as i32 - 44;
    hline(&mut img, MARGIN, W as i32 - MARGIN, fy, p.dim);
    dashed_hline(&mut img, MARGIN, W as i32 - MARGIN, fy + 2, p.dim);
    txt(&mut img, fr, "THINKBOOK-EINK // GITHUB.COM/LIZARDKING00/THINKBOOK-EINK", MARGIN, fy + 10, 22.0, p.dim);
    txt_r(&mut img, fr, "SYS:NOMINAL", W as i32 - MARGIN, fy + 10, 22.0, p.dim);

    img
}

// ---------------------------------------------------------------------------
// System helpers
// ---------------------------------------------------------------------------

fn get_network_speeds(_sys: &System, prev_rx: u64, prev_tx: u64, elapsed_secs: f64) -> (f64, f64, u64, u64) {
    let mut total_rx: u64 = 0;
    let mut total_tx: u64 = 0;
    let networks = Networks::new_with_refreshed_list();
    for (name, data) in &networks {
        if name == "lo" { continue; }
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

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() -> Result<()> {
    let config = Config::load();
    eprintln!("Connecting to E-ink display...");
    let mut it8951 = It8951::connect()?;
    let sys_info = it8951
        .get_system_info()
        .ok_or(anyhow::anyhow!("Failed to get system info"))?;
    let (dw, dh) = (sys_info.width, sys_info.height);
    eprintln!(
        "Connected: {}x{}. Starting server dashboard (Ctrl+C to stop).",
        dw, dh
    );
    eprintln!(
        "Config: theme={}, flip={}",
        if config.is_dark() { "dark" } else { "light" },
        config.is_flipped()
    );

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
        let (rx_speed, tx_speed, total_rx, total_tx) =
            get_network_speeds(&sys, prev_rx, prev_tx, elapsed);
        prev_rx = total_rx;
        prev_tx = total_tx;
        if upload_history.len() == 60 { upload_history.pop_front(); }
        if download_history.len() == 60 { download_history.pop_front(); }
        upload_history.push_back(tx_speed);
        download_history.push_back(rx_speed);

        let cpu_temp = get_cpu_temp();
        let (nc_online, nc_version, nc_latency_ms, nc_url) = check_nextcloud(&config);
        let (nc_active_5m, nc_active_1h, nc_active_24h, nc_app_updates, nc_core_update) =
            fetch_serverinfo(&config);

        let img = render(
            &font_bold,
            &font_reg,
            &sys,
            &upload_history,
            &download_history,
            nc_online,
            &nc_version,
            nc_latency_ms,
            cpu_temp,
            &nc_url,
            nc_active_5m,
            nc_active_1h,
            nc_active_24h,
            nc_app_updates,
            nc_core_update,
            &config,
        );

        let mut prepared = DynamicImage::ImageLuma8(img);
        if config.is_flipped() {
            prepared = DynamicImage::ImageLuma8(imageops::rotate180(&prepared.to_luma8()));
        }

        // Clear with a white (dark mode) or black (light mode) GC16 frame to
        // scrub ghosting, then draw the new frame with DU.
        let clear_pixel = if config.is_dark() { Luma([255u8]) } else { Luma([0u8]) };
        let clear_img = GrayImage::from_pixel(dw, dh, clear_pixel);
        let clear_dyn = DynamicImage::ImageLuma8(clear_img);
        it8951.load_region(&clear_dyn, 0, 0)?;
        it8951.display_region(0, 0, dw, dh, Mode::GC16)?;

        it8951.load_region(&prepared, 0, 0)?;
        it8951.display_region(0, 0, dw, dh, Mode::DU)?;

        eprintln!(
            "[{}] RAM:{:.0}% CPU:{:.0}% TEMP:{:.0}C TX:{} RX:{} NC:{}",
            Local::now().format("%H:%M:%S"),
            sys.used_memory() as f32 / sys.total_memory() as f32 * 100.0,
            sys.global_cpu_info().cpu_usage(),
            cpu_temp,
            format_speed(tx_speed),
            format_speed(rx_speed),
            if nc_online { "ONLINE" } else { "OFFLINE" }
        );

        // Sleep until the start of the next minute
        let now = Local::now();
        let secs_remaining = 60 - now.second();
        let nanos_remaining = 1_000_000_000 - now.nanosecond() % 1_000_000_000;
        let sleep_dur = Duration::from_secs(secs_remaining as u64)
            - Duration::from_nanos(now.nanosecond() as u64 % 1_000_000_000)
            + Duration::from_nanos(nanos_remaining as u64);
        // Clamp to avoid sleeping 0 or negative after arithmetic edge cases
        let sleep_dur = sleep_dur.max(Duration::from_secs(1));
        thread::sleep(sleep_dur);
    }
}
