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
    network_interface: Option<String>,
    enabled_widgets: Option<Vec<String>>,
    nextcloud_container: Option<String>,
    couchdb_url: Option<String>,
    couchdb_user: Option<String>,
    couchdb_password: Option<String>,
    couchdb_databases: Option<Vec<String>>,
    firefly_url: Option<String>,
    firefly_token: Option<String>,
}

impl Config {
    fn load() -> Self {
        let mut config = if Path::new(CONFIG_PATH).exists() {
            match fs::read_to_string(CONFIG_PATH) {
                Ok(contents) => match toml::from_str(&contents) {
                    Ok(config) => config,
                    Err(e) => {
                        eprintln!(
                            "WARNING: failed to parse {} ({}) — falling back to defaults. \
                             The whole config is now ignored, including flip/theme/credentials.",
                            CONFIG_PATH, e
                        );
                        Config::default()
                    }
                },
                Err(e) => {
                    eprintln!("WARNING: failed to read {} ({}) — falling back to defaults.", CONFIG_PATH, e);
                    Config::default()
                }
            }
        } else {
            Config::default()
        };
        config.load_secrets_from_env();
        config
    }

    /// Credentials are meant to live in the systemd unit's EnvironmentFile
    /// (see secrets.env.example), not in server.toml — this only fills in
    /// what TOML didn't already set, so an existing deployment with secrets
    /// still in server.toml keeps working during migration.
    fn load_secrets_from_env(&mut self) {
        let fill = |field: &mut Option<String>, var: &str| {
            if field.is_none() {
                *field = std::env::var(var).ok();
            }
        };
        fill(&mut self.nextcloud_user, "NEXTCLOUD_USER");
        fill(&mut self.nextcloud_password, "NEXTCLOUD_PASSWORD");
        fill(&mut self.nextcloud_token, "NEXTCLOUD_TOKEN");
        fill(&mut self.couchdb_user, "COUCHDB_USER");
        fill(&mut self.couchdb_password, "COUCHDB_PASSWORD");
        fill(&mut self.firefly_token, "FIREFLY_TOKEN");
    }

    fn is_flipped(&self) -> bool {
        self.flip.unwrap_or(false)
    }

    fn is_dark(&self) -> bool {
        matches!(self.theme.as_ref().unwrap_or(&Theme::Dark), Theme::Dark)
    }
}

// ---------------------------------------------------------------------------
// Optional service widgets (lower-right rotating panel)
// ---------------------------------------------------------------------------

// A widget only ever fetches data or draws if its name appears in the
// user's `enabled_widgets` config list. Someone who never adds it there
// pays no runtime cost and sees nothing related to it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum WidgetKind {
    Docker,
    Obsidian,
    Firefly,
}

impl WidgetKind {
    fn parse(name: &str) -> Option<Self> {
        match name.to_lowercase().as_str() {
            "docker" => Some(WidgetKind::Docker),
            "obsidian" => Some(WidgetKind::Obsidian),
            "firefly" => Some(WidgetKind::Firefly),
            _ => None,
        }
    }

    fn label(&self) -> &'static str {
        match self {
            WidgetKind::Docker => "DOCKER",
            WidgetKind::Obsidian => "OBSIDIAN",
            WidgetKind::Firefly => "FIREFLY",
        }
    }
}

fn enabled_widget_kinds(config: &Config) -> Vec<WidgetKind> {
    config
        .enabled_widgets
        .as_ref()
        .map(|list| list.iter().filter_map(|s| WidgetKind::parse(s)).collect())
        .unwrap_or_default()
}

struct DockerStatus {
    running: u32,
    stopped: Vec<String>,
}

/// Shells out to `docker ps`. Returns None if Docker isn't installed, isn't
/// running, or the current user lacks permission — the widget then shows
/// an "unavailable" state instead of drawing anything misleading.
fn get_docker_status() -> Option<DockerStatus> {
    let output = std::process::Command::new("docker")
        .args(["ps", "-a", "--format", "{{.Names}}\t{{.State}}"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut running = 0u32;
    let mut stopped = Vec::new();
    for line in text.lines() {
        let mut parts = line.splitn(2, '\t');
        let name = parts.next().unwrap_or("");
        let state = parts.next().unwrap_or("");
        if state == "running" {
            running += 1;
        } else if !name.is_empty() {
            stopped.push(name.to_string());
        }
    }
    Some(DockerStatus { running, stopped })
}

fn format_stopped(names: &[String]) -> String {
    if names.is_empty() {
        return "0 STOPPED".to_string();
    }
    if names.len() <= 3 {
        format!("{} STOPPED: {}", names.len(), names.join(", "))
    } else {
        format!(
            "{} STOPPED: {}, +{} MORE",
            names.len(),
            names[..3].join(", "),
            names.len() - 3
        )
    }
}

struct ObsidianStatus {
    total_size: u64,
    active_size: u64,
    doc_count: u64,
    doc_del_count: u64,
    compacting: bool,
}

// Warn once the synced vault databases' combined on-disk size crosses this —
// LiveSync keeps full revision history and this host has no compaction
// schedule configured, so unbounded growth is a real, known risk here.
const COUCHDB_SIZE_WARN_BYTES: u64 = 2 * 1024 * 1024 * 1024;
// Or once on-disk size dwarfs the actual live data — a better early signal
// than the absolute threshold above, since it catches bloat regardless of
// how big the vault itself is.
const COUCHDB_BLOAT_RATIO_WARN: f64 = 3.0;

/// Sums CouchDB's per-database stats (`GET /<db>`) across every database
/// listed in `couchdb_databases` — Self-hosted LiveSync names its database
/// per vault, so more than one can legitimately exist.
fn get_obsidian_status(config: &Config) -> Option<ObsidianStatus> {
    let base = config.couchdb_url.as_deref()?.trim_end_matches('/').to_string();
    let dbs = config.couchdb_databases.as_ref()?;
    if dbs.is_empty() {
        return None;
    }
    let client = http_client()?;
    let mut status = ObsidianStatus { total_size: 0, active_size: 0, doc_count: 0, doc_del_count: 0, compacting: false };
    for db in dbs {
        let mut req = client.get(format!("{}/{}", base, db));
        if let Some(user) = config.couchdb_user.as_deref() {
            req = req.basic_auth(user, config.couchdb_password.as_deref());
        }
        let json: serde_json::Value = req.send().ok()?.json().ok()?;
        status.total_size += json["sizes"]["file"].as_u64().unwrap_or(0);
        status.active_size += json["sizes"]["active"].as_u64().unwrap_or(0);
        status.doc_count += json["doc_count"].as_u64().unwrap_or(0);
        status.doc_del_count += json["doc_del_count"].as_u64().unwrap_or(0);
        status.compacting = status.compacting || json["compact_running"].as_bool().unwrap_or(false);
    }
    Some(status)
}

impl ObsidianStatus {
    /// How many times bigger the on-disk file is than the live data it
    /// actually holds — old revisions and tombstones CouchDB hasn't
    /// reclaimed yet. 1.0 means no bloat.
    fn bloat_ratio(&self) -> f64 {
        if self.active_size == 0 { return 1.0; }
        self.total_size as f64 / self.active_size as f64
    }

    fn needs_compaction_warning(&self) -> bool {
        // A freshly-synced, tiny database can have a huge bloat ratio just
        // from initial setup churn — the ratio only means something once
        // there's enough data for it to reflect real accumulated history.
        const MIN_SIZE_FOR_RATIO_CHECK: u64 = 50 * 1024 * 1024;
        self.total_size > COUCHDB_SIZE_WARN_BYTES
            || (self.total_size > MIN_SIZE_FOR_RATIO_CHECK && self.bloat_ratio() > COUCHDB_BLOAT_RATIO_WARN)
    }
}

struct FireflyStatus {
    bills_paid: f64,
    bills_unpaid: f64,
    currency_symbol: String,
}

/// Queries Firefly III's summary/basic endpoint for the current calendar
/// month (verified live: it 400s without start/end params). The response
/// is keyed by currency (e.g. "bills-paid-in-EUR", since Firefly supports
/// multiple currencies), so this scans for the first matching key rather
/// than assuming a currency — and monetary_value is a numeric *string* in
/// the real response, not a JSON number.
fn get_firefly_status(config: &Config) -> Option<FireflyStatus> {
    let base = config.firefly_url.as_deref()?.trim_end_matches('/').to_string();
    let token = config.firefly_token.as_deref()?;
    let client = http_client()?;
    let now = Local::now();
    let start = now.format("%Y-%m-01").to_string();
    let end = now.format("%Y-%m-%d").to_string();
    let url = format!("{}/api/v1/summary/basic?start={}&end={}", base, start, end);
    let json: serde_json::Value = client.get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Accept", "application/json")
        .send().ok()?.json().ok()?;
    let obj = json.as_object()?;
    let paid = obj.iter().find(|(k, _)| k.starts_with("bills-paid-in-"))?.1;
    let unpaid = obj.iter().find(|(k, _)| k.starts_with("bills-unpaid-in-"))?.1;
    let bills_paid = paid["monetary_value"].as_str()?.parse::<f64>().ok()?;
    let bills_unpaid = unpaid["monetary_value"].as_str()?.parse::<f64>().ok()?;
    let currency_symbol = paid["currency_symbol"].as_str().unwrap_or("").to_string();
    Some(FireflyStatus { bills_paid, bills_unpaid, currency_symbol })
}

enum WidgetContent {
    Docker(DockerStatus),
    Obsidian(ObsidianStatus),
    Firefly(FireflyStatus),
}

/// State for the lower-right rotating widget panel: which widgets are
/// enabled, which one is active this render, its fetched data (if any),
/// and how many ticks remain before rotating to the next one.
struct WidgetPanelState {
    enabled: Vec<WidgetKind>,
    active_idx: usize,
    content: Option<WidgetContent>,
    ticks_remaining: u32,
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

/// Truncates `text` with a trailing "..." so it never exceeds `max_w`
/// pixels — a safety net for any label built from variable-length upstream
/// data (server strings, container names, etc.) that could otherwise
/// overflow into a neighboring panel. Returns the text unchanged if it
/// already fits.
fn clip_text(font: &Font, text: &str, size: f32, max_w: i32) -> String {
    let scale = Scale::uniform(size);
    let (w, _) = text_size(scale, font, text);
    if w <= max_w {
        return text.to_string();
    }
    let mut kept = String::new();
    for ch in text.chars() {
        let candidate = format!("{}{}...", kept, ch);
        let (cw, _) = text_size(scale, font, &candidate);
        if cw > max_w { break; }
        kept.push(ch);
    }
    format!("{}...", kept)
}

/// Draws left-aligned text, clipped per `clip_text`.
fn txt_clip(img: &mut GrayImage, font: &Font, text: &str, x: i32, y: i32, size: f32, max_w: i32, color: Luma<u8>) {
    let clipped = clip_text(font, text, size, max_w);
    draw_text_mut(img, color, x, y, Scale::uniform(size), font, &clipped);
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

/// A small faceted-gem glyph (44x56px, evocative of "obsidian" the
/// mineral) drawn as wireframe line segments, matching the dashboard's
/// existing corner-bracket/line-art style. Original geometry, not a
/// reproduction of any application's logo.
fn draw_gem_icon(img: &mut GrayImage, x: i32, y: i32, color: Luma<u8>) {
    let (x, y) = (x as f32, y as f32);
    let top = (x + 22.0, y);
    let right = (x + 44.0, y + 20.0);
    let bottom = (x + 22.0, y + 56.0);
    let left = (x, y + 20.0);
    draw_line_segment_mut(img, top, right, color);
    draw_line_segment_mut(img, right, bottom, color);
    draw_line_segment_mut(img, bottom, left, color);
    draw_line_segment_mut(img, left, top, color);
    draw_line_segment_mut(img, left, right, color);
    draw_line_segment_mut(img, top, bottom, color);
}

/// A warning-triangle glyph (26x24px: outline + exclamation mark) drawn as
/// vector line art rather than the Unicode ⚠ character — this font can't
/// render that glyph at all (same tofu-box issue as the rotation dots and
/// the TIME -> arrow before it), but plain line segments sidestep the
/// problem entirely.
fn draw_warn_triangle(img: &mut GrayImage, x: i32, y: i32, color: Luma<u8>) {
    let (w, h) = (26.0, 24.0);
    let (xf, yf) = (x as f32, y as f32);
    let apex = (xf + w / 2.0, yf);
    let bottom_right = (xf + w, yf + h);
    let bottom_left = (xf, yf + h);
    draw_line_segment_mut(img, apex, bottom_right, color);
    draw_line_segment_mut(img, bottom_right, bottom_left, color);
    draw_line_segment_mut(img, bottom_left, apex, color);
    draw_line_segment_mut(img, (xf + w / 2.0, yf + h * 0.32), (xf + w / 2.0, yf + h * 0.62), color);
    draw_filled_rect_mut(img, Rect::at((xf + w / 2.0 - 1.0) as i32, (yf + h * 0.74) as i32).of_size(2, 2), color);
}

/// A small receipt/bill glyph (40x58px: torn-edge rectangle with a few
/// line-item marks inside), wireframe line art matching the dashboard's
/// existing icon style — original geometry, not any app's logo.
fn draw_receipt_icon(img: &mut GrayImage, x: i32, y: i32, color: Luma<u8>) {
    let (xf, yf) = (x as f32, y as f32);
    draw_line_segment_mut(img, (xf, yf), (xf + 40.0, yf), color);
    draw_line_segment_mut(img, (xf, yf), (xf, yf + 48.0), color);
    draw_line_segment_mut(img, (xf + 40.0, yf), (xf + 40.0, yf + 48.0), color);
    let zigzag = [
        (xf, yf + 48.0), (xf + 10.0, yf + 58.0), (xf + 20.0, yf + 48.0),
        (xf + 30.0, yf + 58.0), (xf + 40.0, yf + 48.0),
    ];
    for pair in zigzag.windows(2) {
        draw_line_segment_mut(img, pair[0], pair[1], color);
    }
    for i in 0..3 {
        let ly = yf + 14.0 + i as f32 * 10.0;
        draw_line_segment_mut(img, (xf + 8.0, ly), (xf + 32.0, ly), color);
    }
}

fn dashed_hline(img: &mut GrayImage, x1: i32, x2: i32, y: i32, color: Luma<u8>) {
    let mut x = x1;
    while x < x2 {
        hline(img, x, (x + 12).min(x2), y, color);
        x += 18;
    }
}

fn draw_graph(img: &mut GrayImage, x: i32, y: i32, w: i32, h: i32, values: &VecDeque<f64>, max_val: f64, p: &Palette) {
    if values.len() < 2 { return; }
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

fn format_count(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.2}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
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

/// Shared blocking HTTP client builder — every widget/status check that
/// talks HTTP (Nextcloud, CouchDB, ...) uses the same short timeout and
/// accepts self-signed certs, since these are all local/home-server
/// endpoints.
fn http_client() -> Option<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(Duration::from_secs(5))
        .build()
        .ok()
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
    let client = match http_client() {
        Some(c) => c,
        None => return (false, String::new(), 0, base),
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

#[derive(Default)]
struct ServerInfo {
    active_5m: u64,
    active_1h: u64,
    active_24h: u64,
    app_updates: u64,
    core_update: bool,
    php_version: String,
    db_type: String,
    db_version: String,
    num_users: u64,
    num_disabled_users: u64,
    num_files: u64,
    shares_user: u64,
    shares_group: u64,
    shares_link: u64,
    free_space: u64,
}

impl ServerInfo {
    /// True once we've actually populated data from a serverinfo response,
    /// as opposed to the all-zero/empty defaults returned when Nextcloud
    /// isn't configured, unreachable, or the JSON shape didn't match.
    fn has_detail(&self) -> bool {
        !self.php_version.is_empty() || self.num_users > 0 || self.num_files > 0
    }
}

/// Fetch rich server info from the Nextcloud serverinfo API.
fn fetch_serverinfo(config: &Config) -> ServerInfo {
    let defaults = ServerInfo::default();
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
    let client = match http_client() {
        Some(c) => c,
        None => return defaults,
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

    // Paths verified against a live serverinfo response (NC 33.0.5.1) rather
    // than guessed — the API has no "apps" object anywhere under
    // nextcloud.system on this version (so an installed-app count isn't
    // available here at all; we show registered users instead), and
    // server.database.version can be a long free-text string (e.g. Postgres's
    // full "PostgreSQL 18.4 on x86_64-pc-linux-musl, compiled by...") rather
    // than a short number, so it's trimmed to its first two words below.
    let php_version = data["server"]["php"]["version"].as_str().unwrap_or("").to_string();
    let db_type = data["server"]["database"]["type"].as_str().unwrap_or("").to_string();
    let db_version_raw = data["server"]["database"]["version"].as_str().unwrap_or("");
    let db_version = db_version_raw.split_whitespace().take(2).collect::<Vec<_>>().join(" ");
    let num_users = data["nextcloud"]["storage"]["num_users"].as_u64().unwrap_or(0);
    let num_disabled_users = data["nextcloud"]["storage"]["num_disabled_users"].as_u64().unwrap_or(0);
    let num_files = data["nextcloud"]["storage"]["num_files"].as_u64().unwrap_or(0);
    let shares_user = data["nextcloud"]["shares"]["num_shares_user"].as_u64().unwrap_or(0);
    let shares_group = data["nextcloud"]["shares"]["num_shares_groups"].as_u64().unwrap_or(0);
    let shares_link = data["nextcloud"]["shares"]["num_shares_link"].as_u64().unwrap_or(0);
    let free_space = data["nextcloud"]["system"]["freespace"].as_u64().unwrap_or(0);

    eprintln!(
        "[serverinfo] users={}/{}/{} app_updates={} core_update={}",
        active_5m, active_1h, active_24h, app_updates, core_update
    );

    ServerInfo {
        active_5m, active_1h, active_24h, app_updates, core_update,
        php_version, db_type, db_version, num_users, num_disabled_users, num_files,
        shares_user, shares_group, shares_link, free_space,
    }
}

/// Runs `occ update:check` inside the Nextcloud container for accurate core
/// and app update status. The serverinfo API doesn't expose this at all on
/// current Nextcloud versions (verified: no "apps"/"update" keys anywhere
/// in the response), so this is the only reliable source — it's the same
/// check the admin UI itself is based on. Only used when `nextcloud_container`
/// is configured; not run every render (see UPDATE_CHECK_INTERVAL_TICKS in
/// main) since it calls out to the Nextcloud app store for every app.
fn check_nextcloud_updates(container: &str) -> Option<(u64, bool)> {
    let output = std::process::Command::new("docker")
        .args(["exec", "--user", "www-data", container, "php", "occ", "update:check"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut core_update = false;
    let mut app_updates = 0u64;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("Nextcloud ") && line.contains("is available") {
            core_update = true;
        } else if line.starts_with("Update for ") && line.contains("is available") {
            app_updates += 1;
        }
    }
    Some((app_updates, core_update))
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
    info: &ServerInfo,
    widget: &WidgetPanelState,
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
    let users_label = if nc_online && (info.active_5m > 0 || info.active_1h > 0 || info.active_24h > 0) {
        format!("USERS: {}/{}/{} (5M/1H/24H)", info.active_5m, info.active_1h, info.active_24h)
    } else {
        "USERS: -/-/- (5M/1H/24H)".to_string()
    };
    txt(&mut img, fr, &users_label, MARGIN, 152, 26.0, p.mid);
    if info.app_updates > 0 {
        draw_warn_triangle(&mut img, 560, 150, p.bright);
        let alert = format!("{} APP UPDATES PENDING", info.app_updates);
        txt(&mut img, fb, &alert, 560 + 34, 150, 26.0, p.bright);
    }
    if info.core_update {
        draw_warn_triangle(&mut img, 1060, 150, p.bright);
        txt(&mut img, fb, "CORE UPDATE PENDING", 1060 + 34, 150, 26.0, p.bright);
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

    // Shared y-axis scale so the two graphs are directly comparable — a real
    // magnitude difference shows as different curve heights, not just in
    // the small TX:/RX: text labels.
    let net_max = upload_history.iter().chain(download_history.iter())
        .cloned().fold(0.0_f64, f64::max).max(1.0);

    // Box top is raised to r2y (instead of r2y+38) so the title/current-speed
    // row sits inside the box like every other panel's header, instead of
    // floating above it where it could crowd or clip against the box border.
    let bx = MARGIN;
    corner_box(&mut img, bx, r2y, graph_w, graph_h + 38, arm, p.dim);
    txt(&mut img, fb, "// UPLOAD", bx + arm + 6, r2y + 8, 26.0, p.mid);
    let cur_up = upload_history.back().cloned().unwrap_or(0.0);
    txt_r(&mut img, fb, &format!("TX: {}", format_speed(cur_up)), bx + graph_w - 10, r2y + 8, 26.0, p.bright);
    draw_graph(&mut img, bx + 4, r2y + 42, graph_w - 8, graph_h - 8, upload_history, net_max, &p);
    txt(&mut img, fr, "SPEED", bx + 10, r2y + 46, 20.0, p.dim);
    txt_r(&mut img, fr, "TIME ->", bx + graph_w - 10, r2y + 38 + graph_h + 4, 20.0, p.dim);
    txt_c(&mut img, fr, "TX MB/S (LAST 60 MIN)", bx + graph_w / 2, r2y + 38 + graph_h + 26, 20.0, p.dim);

    let bx = MARGIN + graph_w + 60;
    corner_box(&mut img, bx, r2y, graph_w, graph_h + 38, arm, p.dim);
    txt(&mut img, fb, "// DOWNLOAD", bx + arm + 6, r2y + 8, 26.0, p.mid);
    let cur_down = download_history.back().cloned().unwrap_or(0.0);
    txt_r(&mut img, fb, &format!("RX: {}", format_speed(cur_down)), bx + graph_w - 10, r2y + 8, 26.0, p.bright);
    draw_graph(&mut img, bx + 4, r2y + 42, graph_w - 8, graph_h - 8, download_history, net_max, &p);
    txt(&mut img, fr, "SPEED", bx + 10, r2y + 46, 20.0, p.dim);
    txt_r(&mut img, fr, "TIME ->", bx + graph_w - 10, r2y + 38 + graph_h + 4, 20.0, p.dim);
    txt_c(&mut img, fr, "RX MB/S (LAST 60 MIN)", bx + graph_w / 2, r2y + 38 + graph_h + 26, 20.0, p.dim);

    // Divider between the graphs row and the panel row below.
    dashed_hline(&mut img, MARGIN, W as i32 - MARGIN, r2y + 38 + graph_h + 56, p.dim);

    // Row 3: static Nextcloud detail panel (left) + rotating widget panel (right).
    // Left is narrower since the Nextcloud URL/config lines sit below it;
    // right extends further down to use that same space, since it has
    // nothing below it. Heights are 12px shorter than before to make room
    // for the new divider above while keeping both panels' bottom edges
    // (and their clearance from the URL summary / footer) unchanged.
    let r3y = r2y + 38 + graph_h + 66;
    let left_w = 620;
    let left_h = 240;
    let gap = 40;
    let right_x = MARGIN + left_w + gap;
    let right_w = (W as i32 - MARGIN) - right_x;
    let right_h = 298;

    // Left: Nextcloud detail (static, always on when Nextcloud is configured).
    // Every line goes through txt_clip so variable-length upstream data
    // (long DB version strings, etc.) can never overflow into the widget
    // panel to its right.
    let bx = MARGIN;
    corner_box(&mut img, bx, r3y, left_w, left_h, arm, p.dim);
    txt(&mut img, fr, "// NEXTCLOUD", bx + 10, r3y + 8, 24.0, p.dim);
    let line_max_w = left_w - 20;
    if info.has_detail() {
        let lx = bx + 10;
        let mut ly = r3y + 46;
        let db = if info.db_type.is_empty() { "-".to_string() } else { info.db_type.to_uppercase() };
        let db_ver = if info.db_version.is_empty() { "-" } else { &info.db_version };
        let php = if info.php_version.is_empty() { "-" } else { &info.php_version };
        let users_line = if info.num_disabled_users > 0 {
            format!("USERS: {} REGISTERED ({} DISABLED)", info.num_users, info.num_disabled_users)
        } else {
            format!("USERS: {} REGISTERED", info.num_users)
        };
        txt_clip(&mut img, fr, &format!("PHP {} / {} {}", php, db, db_ver), lx, ly, 22.0, line_max_w, p.bright); ly += 34;
        txt_clip(&mut img, fr, &users_line, lx, ly, 22.0, line_max_w, p.bright); ly += 34;
        txt_clip(&mut img, fr, &format!("FILES: {}", format_count(info.num_files)), lx, ly, 22.0, line_max_w, p.bright); ly += 34;
        txt_clip(&mut img, fr, &format!("SHARES: {} USR / {} GRP / {} LINK", info.shares_user, info.shares_group, info.shares_link), lx, ly, 22.0, line_max_w, p.bright); ly += 34;
        txt_clip(&mut img, fr, &format!("FREE SPACE: {}", format_bytes(info.free_space)), lx, ly, 22.0, line_max_w, p.bright);
    } else {
        txt(&mut img, fr, "NO SERVERINFO DATA", bx + 10, r3y + 46, 22.0, p.dim);
    }

    // Right: rotating optional-service widget panel
    corner_box(&mut img, right_x, r3y, right_w, right_h, arm, p.dim);
    txt(&mut img, fr, "// SERVICES", right_x + 10, r3y + 8, 24.0, p.dim);
    if widget.enabled.is_empty() {
        txt_c(&mut img, fr, "NO WIDGETS ENABLED", right_x + right_w / 2, r3y + right_h / 2, 22.0, p.dim);
    } else {
        // ASCII markers, not Unicode bullet glyphs — this font/rendering
        // pipeline has previously dropped non-ASCII symbols entirely
        // (see the TIME -> arrow fix), rendering as empty boxes.
        let dots: String = widget.enabled.iter().enumerate()
            .map(|(i, w)| format!("{} {}", if i == widget.active_idx { "*" } else { "o" }, w.label()))
            .collect::<Vec<_>>().join("  ");
        let dots = clip_text(fr, &dots, 22.0, right_w / 2);
        txt_r(&mut img, fr, &dots, right_x + right_w - 10, r3y + 8, 22.0, p.bright);

        let cx = right_x + right_w / 2;
        match &widget.content {
            Some(WidgetContent::Docker(status)) => {
                txt_c(&mut img, fb, &format!("{} CONTAINERS RUNNING", status.running), cx, r3y + 100, 56.0, p.bright);
                let stopped_color = if status.stopped.is_empty() { p.mid } else { p.bright };
                txt_clip(&mut img, fr, &format_stopped(&status.stopped), right_x + 10, r3y + 190, 26.0, right_w - 20, stopped_color);
            }
            Some(WidgetContent::Obsidian(status)) => {
                draw_gem_icon(&mut img, right_x + 20, r3y + 40, p.mid);
                txt_c(&mut img, fb, &format!("DB SIZE: {}", format_bytes(status.total_size)), cx, r3y + 100, 56.0, p.bright);
                let detail = format!(
                    "{} DOCS ({} DEL) - {} LIVE ({:.1}X)",
                    status.doc_count, status.doc_del_count,
                    format_bytes(status.active_size), status.bloat_ratio(),
                );
                let detail = clip_text(fr, &detail, 24.0, right_w - 20);
                txt_c(&mut img, fr, &detail, cx, r3y + 190, 24.0, p.mid);
                if status.compacting {
                    txt_c(&mut img, fr, "COMPACTING...", cx, r3y + 232, 24.0, p.mid);
                } else if status.needs_compaction_warning() {
                    txt_c(&mut img, fr, "COMPACT RECOMMENDED", cx, r3y + 232, 24.0, p.bright);
                }
            }
            Some(WidgetContent::Firefly(status)) => {
                draw_receipt_icon(&mut img, right_x + 20, r3y + 40, p.mid);
                let unpaid = format!("{}{:.2} UNPAID", status.currency_symbol, status.bills_unpaid);
                let unpaid = clip_text(fb, &unpaid, 56.0, right_w - 20);
                txt_c(&mut img, fb, &unpaid, cx, r3y + 100, 56.0, p.bright);
                let paid = format!("{}{:.2} PAID THIS MONTH", status.currency_symbol, status.bills_paid);
                let paid = clip_text(fr, &paid, 24.0, right_w - 20);
                txt_c(&mut img, fr, &paid, cx, r3y + 190, 24.0, p.mid);
            }
            None => {
                txt_c(&mut img, fr, &format!("{}: UNAVAILABLE", widget.enabled[widget.active_idx].label()), cx, r3y + right_h / 2, 26.0, p.dim);
            }
        }

        let next = widget.enabled[(widget.active_idx + 1) % widget.enabled.len()];
        let caption = format!(
            "{} ({}/{}) - NEXT: {} IN {}M",
            widget.enabled[widget.active_idx].label(),
            widget.active_idx + 1,
            widget.enabled.len(),
            next.label(),
            widget.ticks_remaining,
        );
        txt_c(&mut img, fr, &caption, cx, r3y + right_h - 28, 18.0, p.dim);
    }

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

// Interfaces that never carry real uplink traffic: loopback and the virtual
// bridge/veth pairs Docker creates per-container. Their traffic is symmetric
// by construction (what leaves a container's eth0 enters its veth peer), so
// summing them alongside the real NIC drowns real up/down asymmetry and
// makes the two graphs look identical.
const VIRTUAL_IFACE_PREFIXES: &[&str] = &["lo", "docker", "veth", "br-", "virbr", "tun", "tap"];

fn get_network_speeds(
    _sys: &System,
    prev_rx: u64,
    prev_tx: u64,
    elapsed_secs: f64,
    iface_filter: Option<&str>,
) -> (f64, f64, u64, u64) {
    let mut total_rx: u64 = 0;
    let mut total_tx: u64 = 0;
    let networks = Networks::new_with_refreshed_list();
    for (name, data) in &networks {
        let include = match iface_filter {
            Some(wanted) => name == wanted,
            None => !VIRTUAL_IFACE_PREFIXES.iter().any(|p| name.starts_with(p)),
        };
        if !include { continue; }
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
    let mut widget_tick: u32 = 0;
    const WIDGET_ROTATE_TICKS: u32 = 5; // ~5 minutes, one render loop tick ≈ 1 minute
    let mut update_check_tick: u32 = 0;
    let mut cached_updates: (u64, bool) = (0, false);
    // occ update:check calls out to the Nextcloud app store for every
    // installed app — too expensive to run every render (~once/minute).
    const UPDATE_CHECK_INTERVAL_TICKS: u32 = 60; // ~once an hour

    loop {
        sys.refresh_all();
        let elapsed = last_tick.elapsed().as_secs_f64().max(1.0);
        last_tick = std::time::Instant::now();
        let (rx_speed, tx_speed, total_rx, total_tx) =
            get_network_speeds(&sys, prev_rx, prev_tx, elapsed, config.network_interface.as_deref());
        prev_rx = total_rx;
        prev_tx = total_tx;
        if upload_history.len() == 60 { upload_history.pop_front(); }
        if download_history.len() == 60 { download_history.pop_front(); }
        upload_history.push_back(tx_speed);
        download_history.push_back(rx_speed);

        let cpu_temp = get_cpu_temp();
        let (nc_online, nc_version, nc_latency_ms, nc_url) = check_nextcloud(&config);
        let mut info = fetch_serverinfo(&config);

        if let Some(container) = config.nextcloud_container.as_deref() {
            if update_check_tick % UPDATE_CHECK_INTERVAL_TICKS == 0 {
                if let Some(result) = check_nextcloud_updates(container) {
                    cached_updates = result;
                }
            }
            info.app_updates = cached_updates.0;
            info.core_update = cached_updates.1;
        }
        update_check_tick = update_check_tick.wrapping_add(1);

        let enabled = enabled_widget_kinds(&config);
        let widget = if enabled.is_empty() {
            WidgetPanelState { enabled, active_idx: 0, content: None, ticks_remaining: 0 }
        } else {
            let active_idx = (widget_tick / WIDGET_ROTATE_TICKS) as usize % enabled.len();
            let ticks_remaining = WIDGET_ROTATE_TICKS - (widget_tick % WIDGET_ROTATE_TICKS);
            let content = match enabled[active_idx] {
                WidgetKind::Docker => get_docker_status().map(WidgetContent::Docker),
                WidgetKind::Obsidian => get_obsidian_status(&config).map(WidgetContent::Obsidian),
                WidgetKind::Firefly => get_firefly_status(&config).map(WidgetContent::Firefly),
            };
            WidgetPanelState { enabled, active_idx, content, ticks_remaining }
        };
        widget_tick = widget_tick.wrapping_add(1);

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
            &info,
            &widget,
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
