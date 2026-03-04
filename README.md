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
|                      23:41                               |
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
- **Status bar**: Nextcloud online/offline state, total users (optional), system uptime
- **Stats row**: RAM usage, root disk usage, CPU load, CPU temperature
- **Graphs**: upload/download network history

The dashboard **updates once per minute**.

To minimise E‑ink ghosting while keeping updates reliable:

- On each refresh `eink-server` first draws a **white frame in GC16 mode** to scrub previous content.
- It then renders the new dashboard frame in **DU mode**, which matches the behaviour of `eink-clock` for fast, low-flicker updates.

This means you may see a brief white flash once per minute, but the resulting image is much clearer and does not accumulate previous images (for example, after using `setbackside` to show a photo).

Rough layout (simplified ASCII preview):

```
+--------------------------------------------------------------------------------------+
| SYS://NEXTCLOUD-NODE                                  10:32              04.03.2026 |
|--------------------------------------------------------------------------------------|
| [ NEXTCLOUD: ONLINE ]    TOTAL USERS: 3      UPTIME: 2D 04H 13M   UPDATED: 10:32    |
|--------------------------------------------------------------------------------------|
|  // RAM        |  // DISK       |  // CPU        |  // TEMP                          |
|  [######    ]  |  [#####     ]  |      23%       |        55°C                       |
|                |                |   LOAD BAR     |        CPU TEMP                   |
|--------------------------------------------------------------------------------------|
| // UPLOAD                         // DOWNLOAD                                       |
|  TX: 1.2 MB/S                     RX: 3.4 MB/S                                      |
|  [ upload graph over time ]       [ download graph over time ]                      |
|        SPEED                                SPEED                                   |
|        TIME →                              TIME →                                   |
|  TX MB/S (LAST 60 MIN)            RX MB/S (LAST 60 MIN)                             |
|                                                                                      |
|  NEXTCLOUD URL: nextcloud.example.com                                               |
|  CFG: /etc/thinkbook-eink/server.toml                                               |
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
`eink-server` combines both: a GC16 white wipe to clear ghosts, followed by DU for the actual dashboard frame.

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
