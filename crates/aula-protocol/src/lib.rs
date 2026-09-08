//! USB HID protocol for AULA RGB keyboards.
//!
//! Currently implements the **AULA F75** (`258a:010c`, Sinowealth 8051) in
//! wired USB-C mode. The protocol was reverse-engineered from USBPcap captures
//! of the vendor driver; see `docs/PROTOCOL.md` for the full write-up.
//!
//! # Quick start
//!
//! ```no_run
//! use aula_protocol::{f75::F75, device::{Frame, RgbDevice}, color::Rgb};
//!
//! let mut kb = F75::open()?;
//! kb.ensure_per_key_mode()?;                       // once, before streaming
//! let frame = Frame::solid(kb.led_count(), Rgb::new(255, 0, 0));
//! kb.set_static(&frame)?;                          // whole board red
//! # Ok::<(), aula_protocol::Error>(())
//! ```
//!
//! # Rules this crate enforces for you
//!
//! These were expensive to discover and are easy to regress:
//!
//! * **Static and streaming are different commands with different payload
//!   layouts.** [`RgbDevice::set_static`] is planar and persists;
//!   [`RgbDevice::stream`] is interleaved and is the only safe path for
//!   animation. Streaming over the static path blanks the entire board,
//!   charging indicator included.
//! * **Config writes are disruptive.** They drop the board out of per-key mode
//!   and trigger an asynchronous repaint that overwrites in-flight frames. The
//!   config write is private; only [`RgbDevice::ensure_per_key_mode`] performs
//!   one, and it waits for the repaint to settle.
//! * **Config reads are validated** against the `5A A5` signature. The firmware
//!   returns truncated all-zero buffers while busy, and writing one back would
//!   corrupt the config block.
//! * **Write rate is capped.** Writing faster than the firmware can absorb
//!   starves its key-scanning loop and the keyboard stops responding to
//!   keypresses until it is replugged.

pub mod color;
pub mod device;
pub mod f75;
pub mod transport;

pub use color::{ChannelOrder, Rgb};
pub use device::{Frame, KeyPos, RgbDevice};
pub use f75::F75;

/// Errors this crate can produce.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("HID error: {0}")]
    Hid(#[from] hidapi::HidError),

    #[error(
        "no AULA keyboard found at {vid:04x}:{pid:04x}. \
         This interface only exists in WIRED USB-C mode — a 2.4GHz dongle or \
         Bluetooth link will not work. Set the side switch to wired, plug in \
         the cable, and retry."
    )]
    NotFound { vid: u16, pid: u16 },

    #[error(
        "found the keyboard but none of its {vendor_collections} vendor \
         collection(s) accepted the RGB protocol{}",
        .last.as_ref().map(|e| format!(" (last error: {e})")).unwrap_or_default()
    )]
    NoRgbInterface {
        vendor_collections: usize,
        last: Option<String>,
    },

    #[error(
        "config read failed: response missing the 5A A5 signature (got {:02x} {:02x}). \
         The firmware returns truncated buffers while busy — let the keyboard \
         settle, avoid pressing the mode knob, and retry.",
        .magic[0], .magic[1]
    )]
    BadConfigRead { magic: [u8; 2] },

    #[error(
        "refused to write a config block that fails the 5A A5 signature check — \
         this would corrupt the keyboard's stored settings"
    )]
    RefusedConfigWrite,
}

pub type Result<T> = std::result::Result<T, Error>;
