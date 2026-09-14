# thinkbook-eink

A Linux driver and CLI toolkit for the E-ink lid display on the **Lenovo ThinkBook Plus Gen 1 (20TG)**.

As far as I know, this is the first working open-source Linux driver for this hardware. Lenovo only ever released a Windows driver, and the display has no official Linux support.

---

## How it was discovered

Running `lsusb` revealed:

```
Bus 001 Device 005: ID 048d:8951 Integrated Technology Express, Inc. ITE T-CON
```

The **ITE IT8951** is a well-documented E-ink timing controller also used in Waveshare's Raspberry Pi E-ink displays. It communicates over USB using custom SCSI commands wrapped in standard USB Bulk Transfer (Command Block Wrapper / Command Status Wrapper protocol).

The vendor ID `048d` and product ID `8951` are hardcoded in the IT8951's USB descriptor. Once identified, it was possible to use an existing Rust library ([rust-it8951](https://github.com/faassen/rust-it8951)) to probe the device — and it responded immediately with the correct resolution (1920x1080) and firmware information. The protocol is fully compatible out of the box.

---

## Hardware compatibility

| Model | Status |
|-------|--------|
| ThinkBook Plus Gen 1 (20TG) | Confirmed working |
| ThinkBook Plus Gen 2 | Unknown — uses different hardware, may work |
| ThinkBook Plus Gen 4 | See [Tinta4Plus](https://github.com/nickcoutsos/thinkbook-eink) |

The display controller reports:
- Resolution: **1920x1080**
- Controller: **ITE IT8951**
- USB endpoints: `0x81` (IN), `0x02` (OUT)
- Standard commands: 12, Extended commands: 44

---

## Requirements

- Ubuntu/Debian-based Linux (tested on Ubuntu 25.10)
- Rust toolchain (`curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`)
- Build tools (`sudo apt install build-essential`)
- Font for clock (`sudo apt install fonts-ubuntu` or `fonts-dejavu`)

---

## Installation

```bash
git clone https://github.com/LizardKing00/thinkbook-eink.git
cd thinkbook-eink
bash install.sh
```

Then **log out and back in** for the udev group change to take effect. After that all commands work without sudo.

To run `eink-server` automatically on boot, install it as a systemd service:

```bash
# Copy and edit the service file (replace REPLACE_ME_WITH_YOUR_USERNAME)
sudo cp eink-server.service /etc/systemd/system/
sudo nano /etc/systemd/system/eink-server.service

sudo systemctl daemon-reload
sudo systemctl enable --now eink-server
```

To update the binary while the service is running, stop it first — the OS will refuse to overwrite a running executable:

```bash
sudo systemctl stop eink-server
sudo cp target/release/eink-server /usr/local/bin/eink-server
sudo systemctl start eink-server
```

---

## Usage

### Set a static image on the lid display

```bash
setbackside ~/Pictures/photo.jpg
setbackside ~/Pictures/wallpaper.png
```

Accepts any format supported by the `image` crate: JPEG, PNG, BMP, WebP, TIFF. The image is automatically resized, cropped, and converted to greyscale to fit the 1920x1080 display.

### Live clock

```bash
eink-clock
```

Displays a large HH:MM clock with the current date, updating every minute. Press Ctrl+C to stop (the last rendered clock face remains on screen — E-ink is non-volatile).

Rough layout (1920x1080 E-ink):

```
+----------------------------------------------------------+
|                                                          |
|                         23:41                            |
|                                                          |
|                       2026-03-04                         |
|                                                          |
|                                                          |
|                                                          |
+----------------------------------------------------------+
```

### Server dashboard (system stats)

```bash
eink-server
```

Shows a full-screen “server dashboard” with:

- **Header**: hostname label, current time and date
- **Status bar (row 1)**: Nextcloud online/offline state, NC version, latency, system uptime
- **Status bar (row 2)**: Active Nextcloud users (last 5 min / 1 hour / 24 hours), update alerts
- **Update alerts**: Warning when Nextcloud app updates or a core update are available (shown with ⚠ symbol)
- **Stats row**: RAM usage, root disk usage, CPU load, CPU temperature
- **Graphs**: upload/download network history, on a shared scale
- **Nextcloud detail panel**: PHP/database version, registered user count, file count, share breakdown, free space — pulled from the same serverinfo call used for active users, so it's free once you've already set up `nextcloud_token`
- **Service widget panel**: optional, rotates every ~5 minutes through whichever services you enable in `enabled_widgets` (see [Configuration](#configuration)) — empty and invisible if you enable none

The dashboard **updates once per minute**.

To minimise E‑ink ghosting while keeping updates reliable:

- On each refresh `eink-server` first draws a **GC16 clear frame** to scrub previous content (white in dark theme, black in light theme).
- It then renders the new dashboard frame in **DU mode**, which matches the behaviour of `eink-clock` for fast, low-flicker updates.

This means you may see a brief flash once per minute, but the resulting image is much clearer and does not accumulate previous images (for example, after using `setbackside` to show a photo).

Rough layout (simplified ASCII preview):

```
+--------------------------------------------------------------------------------------+
| SYS://NEXTCLOUD-NODE                                  10:32              04.03.2026  |
|--------------------------------------------------------------------------------------|
| [ NEXTCLOUD: ONLINE ]  NC VER: 31.0.0  LATENCY: 42MS          UPTIME: 2D 04H 13M   |
| USERS: 1/3/5 (5M/1H/24H)  ⚠ 3 APP UPDATES PENDING     ⚠ CORE UPDATE PENDING       |
|--------------------------------------------------------------------------------------|
|  // RAM        |  // DISK       |  // CPU        |  // TEMP                          |
|                |                |      23%       |        55°C                       |
|                |                |   LOAD BAR     |        CPU TEMP                   |
|--------------------------------------------------------------------------------------|
| // UPLOAD                         // DOWNLOAD                                        |
|  TX: 1.2 MB/S                     RX: 3.4 MB/S                                       |
|  [ upload graph over time ]       [ download graph over time ]                       |
|        SPEED                                SPEED                                    |
|        TIME →                              TIME →                                    |
|  TX MB/S (LAST 60 MIN)            RX MB/S (LAST 60 MIN)                              |
|--------------------------------------------------------------------------------------|
| // NEXTCLOUD             |  // SERVICES                              * DOCKER o... |
|  PHP 8.3.1 / PGSQL 18.4  |                                                           |
|  USERS: 3 REGISTERED     |             24 CONTAINERS RUNNING                        |
|  FILES: 14.7K            |                  0 STOPPED                               |
|  SHARES: 6/0/0           |                                                           |
|  FREE SPACE: 132.6 GB    |        DOCKER (1/1) - NEXT: DOCKER IN 5M                 |
|  NEXTCLOUD URL: nextcloud.example.com                                                |
|  CFG: /etc/thinkbook-eink/server.toml                                                |
+--------------------------------------------------------------------------------------+
```

### Display info

```bash
eink-info
```

Prints hardware and firmware information:

```
Vendor:    Generic
Product:   Storage RamDisc
Revision:  1.00
Resolution: 1920x1080
Firmware:  v65538
```

---

## Display modes

The driver exposes the following IT8951 refresh modes:

| Mode | Quality | Speed | Best for |
|------|---------|-------|----------|
| `GC16` | Full 16-level greyscale | Slow | Photos, detailed images |
| `DU` | Black and white only | Fast | Text, clock updates |
| `A2` | 2-bit | Very fast | Animations |
| `Init` | Blank flash | — | Clearing between images |

`setbackside` uses `GC16` (best quality). `eink-clock` uses `DU` (fast, minimal flicker).  
`eink-server` combines both: a GC16 clear frame to scrub ghosts (colour depends on theme), followed by DU for the actual dashboard frame.

---

## Configuration

All tools read `/etc/thinkbook-eink/server.toml` at startup. All keys are optional — omitting them keeps the defaults.

Credentials (Nextcloud/CouchDB user/password/token) can also go in
`/etc/thinkbook-eink/secrets.env` instead — see [Secrets](#secrets) below.
That's the recommended place for them; server.toml is readable by the
`plugdev` group, secrets.env is root-only.

```toml
# Rotate the display 180 degrees.
# Useful if the laptop is mounted upside down or the lid is physically inverted.
# Default: false
#flip = true

# Colour theme.
# "dark"  = dark background, bright text (default)
# "light" = light background, dark text
#theme = "light"

# Network interface to measure for the upload/download graphs — eink-server only
# Default: auto-exclude loopback and virtual interfaces (docker*, veth*, br-*,
# virbr*, tun*, tap*) and sum whatever real interfaces remain. Set this to pin
# exactly one physical uplink (useful if you run Docker, since bridge/veth
# traffic is symmetric and can otherwise still skew the graphs) or to override
# auto-detection. Run `ip -o link show` to list interface names.
#network_interface = "wlp0s20f3"

# Nextcloud URL (no trailing slash) — eink-server only
#nextcloud_url = "https://localhost"

# Nextcloud credentials for version/latency checks — eink-server only
# Leave empty to skip; online/offline detection still works via status.php.
#nextcloud_user = "admin"
#nextcloud_password = "your-password-here"

# Nextcloud app token (recommended over password) — eink-server only
# Generate one in Nextcloud: Settings -> Security -> Devices & sessions.
# When set, nextcloud_password is ignored for status.php checks.
# For serverinfo API (active users, app updates), this token is sent via
# the NC-Token header. Set it in Nextcloud with:
#   occ config:app:set serverinfo token --value YOUR_TOKEN
#nextcloud_token = "xxxxx-xxxxx-xxxxx-xxxxx-xxxxx"

# Docker container running Nextcloud — eink-server only.
# When set, the app/core update alerts are driven by `occ update:check` run
# inside that container (checked roughly once an hour) instead of the
# serverinfo API, which doesn't expose update status on current Nextcloud
# versions. Requires the Docker CLI and permission to use it.
#nextcloud_container = "nextcloud-aio-nextcloud"

# Optional service widgets — eink-server only.
# Shown one at a time in the lower-right panel, rotating every ~5 minutes.
# A widget only runs (and only ever fires requests) if it's listed here;
# leave unset and the panel shows "NO WIDGETS ENABLED" and nothing runs.
#
# Available widgets:
#   "docker"   - running/stopped container counts via `docker ps`. Requires
#                the Docker CLI and permission to use it (the user running
#                eink-server must be in the `docker` group, or equivalent).
#   "obsidian" - combined disk size / document counts for your Obsidian
#                Self-hosted LiveSync vault database(s), queried directly
#                from CouchDB. Warns once the combined size passes 2GB.
#                Requires couchdb_url and couchdb_databases below.
#   "firefly"  - bills paid/unpaid this calendar month from Firefly III's
#                summary API. Requires firefly_url and a Personal Access
#                Token below.
#enabled_widgets = ["docker", "obsidian", "firefly"]

# CouchDB connection for the "obsidian" widget — eink-server only (no
# trailing slash on the URL).
#couchdb_url = "https://obsidian-sync.example.com"
#couchdb_user = "admin"
#couchdb_password = "your-couchdb-password"
# Database name(s) to report on — Self-hosted LiveSync names one database
# per synced vault, so list every vault you want included. Run
# `curl -u user:pass https://your-couchdb-url/_all_dbs` to see what exists
# (ignore "_users" and "_replicator", those are CouchDB's own).
#couchdb_databases = ["obsidian"]

# Firefly III connection for the "firefly" widget — eink-server only (no
# trailing slash on the URL). Generate a token in Firefly:
# Options -> Profile -> OAuth -> Personal Access Tokens.
#firefly_url = "https://firefly.example.com"
#firefly_token = "your-personal-access-token"
```

A commented-out example is included in `server.toml.example`.

### Secrets

Credentials are better kept out of `server.toml` (world-readable by the
`plugdev` group) and instead placed in `/etc/thinkbook-eink/secrets.env`
(root-only), which the systemd unit loads via `EnvironmentFile=` before
starting `eink-server`:

```bash
sudo install -m 600 -o root -g root secrets.env.example /etc/thinkbook-eink/secrets.env
sudo nano /etc/thinkbook-eink/secrets.env   # fill in real values
sudo systemctl restart eink-server
```

Supported variables — see `secrets.env.example` for the full annotated
list:

```
NEXTCLOUD_USER=admin
NEXTCLOUD_PASSWORD=...
NEXTCLOUD_TOKEN=...
COUCHDB_USER=admin
COUCHDB_PASSWORD=...
FIREFLY_TOKEN=...
```

Anything set in `server.toml`'s `nextcloud_user`/`nextcloud_password`/
`nextcloud_token`/`couchdb_user`/`couchdb_password` still works and takes
priority if both are set — this is only additive, so an existing setup
with credentials already in `server.toml` isn't broken by upgrading.

The `secrets.env` file is entirely optional: if it doesn't exist, systemd
just starts the service without it (note the leading `-` on
`EnvironmentFile=-...` in `systemd/eink-server.service`), and everything
falls back to whatever's in `server.toml`.

### Applying config changes

If `eink-server` is running as a systemd service, restart it after editing the config:

```bash
sudo systemctl restart eink-server
```

To follow the logs and confirm the new settings were picked up:

```bash
journalctl -u eink-server -f
```

The startup line will show the active theme and flip state, for example:

```
Config: theme=light, flip=true
```

If `server.toml` has a syntax error (including a duplicate key — TOML
doesn't allow setting the same key twice, e.g. from pasting a config
snippet in more than once) it logs a `WARNING: failed to parse ...` line
and falls back to all defaults, which means every setting is ignored, not
just the broken one. Check `journalctl -u eink-server -n 20` if the
dashboard suddenly looks like it reset to defaults after an edit.

---

## Without sudo (udev rule)

The install script sets this up automatically. To do it manually:

```bash
sudo cp udev/99-thinkbook-eink.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules
sudo udevadm trigger
sudo usermod -aG plugdev $USER
# log out and back in
```

---

## Using as a library

Add to your `Cargo.toml`:

```toml
[dependencies]
thinkbook-eink = { git = "https://github.com/LizardKing00/thinkbook-eink" }
```

```rust
use thinkbook_eink::{Display, Mode};

fn main() -> anyhow::Result<()> {
    let mut display = Display::connect()?;

    println!("{}", display.info());

    let img = image::open("photo.png")?;
    display.clear()?;
    display.show(&img, Mode::GC16)?;

    Ok(())
}
```

---

## How it works

The IT8951 exposes itself as a USB Mass Storage device (hence the `Generic Storage RamDisc` product string). It accepts custom SCSI commands over standard USB Bulk Transfer endpoints:

1. **CBW** (Command Block Wrapper) — initiates a command with direction and length
2. **Data phase** — image data is chunked into 60KB transfers (IT8951 USB limit)
3. **CSW** (Command Status Wrapper) — confirms completion

Key commands used:

| Command | Opcode | Purpose |
|---------|--------|---------|
| `INQUIRY` | `0x12` | Get vendor/product/revision strings |
| `GET_SYS` | `0xfe...0x80` | Get resolution, firmware, buffer addresses |
| `LD_IMAGE_AREA` | `0xfe...0xa2` | Load image data into framebuffer |
| `DPY_AREA` | `0xfe...0x94` | Trigger display refresh |
| `PMIC_CONTROL` | `0xfe...0xa3` | Power on/off |

---

## License

MIT — see [LICENSE](LICENSE)

---

## Credits

- Protocol reverse engineering based on [rust-it8951](https://github.com/faassen/rust-it8951) by Martijn Faassen
- ITE IT8951 USB Programming Guide (public documentation)
- Discovered and adapted for the ThinkBook Plus Gen 1 by [LizardKing00](https://github.com/LizardKing00)

## Aditional Advice 

[This great 3D-printable wall mount works well with Thinkbooks and allows an unobstructed view of the e-ink display.](https://www.printables.com/model/1559980-laptop-wall-mount)