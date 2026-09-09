//! USB HID protocol for AULA RGB keyboards.
//!
//! Currently implements the **AULA F75** (`258a:010c`, Sinowealth 8051). The
//! protocol was reverse-engineered from USBPcap captures of the vendor driver;
//! see `docs/PROTOCOL.md` for the full write-up.
//!
//! A 2.4 GHz receiver is a **separate USB device** with the receiver chipset's
//! own VID/PID, so it cannot be found by looking for the keyboard's id. It is
//! found by scanning instead — see [`f75::F75::discover`] and [`ScanOptions`].
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
//! * **Write rate is capped, per link.** Writing faster than the firmware can
//!   absorb starves its key-scanning loop and the keyboard stops responding to
//!   keypresses until it is replugged. The wireless cap is deliberately slower
//!   than the wired one.
//! * **Discovery is read-only.** It opens only collections the OS has not
//!   claimed, and identifies a board by reading its config block — never by
//!   trying unknown command bytes, which could write firmware.

pub mod color;
pub mod device;
pub mod dongle;
pub mod f75;
pub mod keyboard;
pub mod transport;

pub use color::{ChannelOrder, Rgb};
pub use device::{DeviceId, Frame, KeyPos, Link, ParseDeviceIdError, RgbDevice};
pub use f75::F75;
pub use keyboard::Keyboard;
pub use transport::{Choice, Confidence, DeviceCandidate, ScanOptions};

/// Errors this crate can produce.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("HID error: {0}")]
    Hid(#[from] hidapi::HidError),

    #[error(
        "no AULA keyboard found (checked {searched} known device id(s)). \
         If the keyboard is on its 2.4 GHz receiver: the receiver is a separate \
         USB device with its own id, so scan for it — in keylux, Keyboard ▸ Scan \
         for receivers; from the CLI, `cargo run --example smoke -- scan --deep`. \
         Bluetooth does not expose this interface."
    )]
    NotFound { searched: usize },

    #[error(
        "found matching hardware but none of its {vendor_collections} vendor \
         collection(s) accepted the RGB protocol{}{}",
        .last.as_ref().map(|e| format!(" (last error: {e})")).unwrap_or_default(),
        // A collection that will not open at all is the signature of a missing
        // udev rule, and the bare message gives a Linux user nothing to act on.
        if cfg!(target_os = "linux") {
            " — on Linux, a collection that will not open usually means the udev \
             rule is missing; see packaging/99-aula.rules"
        } else {
            ""
        }
    )]
    NoRgbInterface {
        vendor_collections: usize,
        last: Option<String>,
    },

    #[error("the device at {path} is no longer there — it was probably unplugged")]
    PathGone { path: String },

    #[error(
        "refused a config write on {id}, which nothing has confirmed as an AULA \
         board. A config write is non-volatile and would change stored keyboard \
         settings. Verify the device first; streaming colours to it is safe and \
         stays available meanwhile."
    )]
    UnverifiedDevice { id: DeviceId },

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

#[cfg(test)]
mod tests {
    use super::*;

    /// The old message told users outright that a 2.4 GHz dongle "will not
    /// work". That was the belief this crate was built on, and it is now wrong
    /// — so guard against it creeping back in a copy-paste.
    #[test]
    fn not_found_no_longer_claims_wireless_is_impossible() {
        let msg = Error::NotFound { searched: 1 }.to_string();
        assert!(!msg.contains("will not work"), "{msg}");
        assert!(!msg.contains("WIRED USB-C"), "{msg}");
        assert!(
            msg.contains("scan"),
            "it should say how to find a receiver: {msg}"
        );
    }
}
