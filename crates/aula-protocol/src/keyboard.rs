//! One keyboard, whichever way it is attached.
//!
//! The wired board and the 2.4 GHz receiver are different devices speaking
//! different protocols, so they are separate drivers. Callers that just want
//! "the keyboard" should not have to care which — that is what this is for.

use crate::color::ChannelOrder;
use crate::device::{DeviceId, Frame, KeyPos, Link, RgbDevice};
use crate::dongle::{self, Dongle};
use crate::f75::F75;
use crate::transport::{self, Choice, DeviceCandidate, ScanOptions};
use crate::{Error, Result};

/// A keyboard on either link.
pub enum Keyboard {
    Wired(F75),
    Wireless(Dongle),
}

impl Keyboard {
    /// Every keyboard reachable right now, on either link.
    ///
    /// Wired candidates are found by probing and confirming a config read;
    /// receivers cannot be probed at all and are matched by id, so the two
    /// searches are genuinely different and are simply concatenated.
    pub fn discover(opts: &ScanOptions) -> Result<Vec<DeviceCandidate>> {
        let mut found = F75::discover(opts)?;
        found.extend(dongle::discover(&opts.allow)?);
        Ok(found)
    }

    pub fn open_candidate(c: &DeviceCandidate, order: ChannelOrder) -> Result<Self> {
        match c.link {
            Link::Dongle => Dongle::open_candidate(c).map(Keyboard::Wireless),
            _ => F75::open_candidate(c, order).map(Keyboard::Wired),
        }
    }

    /// Open the best keyboard available: wired first, then a receiver.
    pub fn open_best(opts: &ScanOptions) -> Result<Self> {
        let found = Self::discover(opts)?;
        match transport::choose(&found, None) {
            Choice::Use(i) => Self::open_candidate(&found[i], ChannelOrder::Rgb),
            _ => Err(Error::NotFound {
                searched: crate::f75::protocol::KNOWN.len()
                    + dongle::KNOWN.len()
                    + opts.allow.len(),
            }),
        }
    }

    /// Open one specific keyboard, by id.
    pub fn open_id(id: DeviceId, opts: &ScanOptions) -> Result<Self> {
        let found = Self::discover(opts)?;
        match transport::choose(&found, Some(id)) {
            Choice::Use(i) => Self::open_candidate(&found[i], ChannelOrder::Rgb),
            _ => Err(Error::NotFound {
                searched: crate::f75::protocol::KNOWN.len()
                    + dongle::KNOWN.len()
                    + opts.allow.len(),
            }),
        }
    }

    pub fn id(&self) -> DeviceId {
        match self {
            Keyboard::Wired(k) => k.id(),
            Keyboard::Wireless(k) => k.id(),
        }
    }

    pub fn link(&self) -> Link {
        match self {
            Keyboard::Wired(k) => k.link(),
            Keyboard::Wireless(_) => Link::Dongle,
        }
    }

    pub fn hid_path(&self) -> &str {
        match self {
            Keyboard::Wired(k) => k.hid_path(),
            Keyboard::Wireless(k) => k.hid_path(),
        }
    }

    /// Whether this keyboard will accept a lighting-mode change.
    ///
    /// Both links have a config block: the receiver carries the same 128-byte
    /// block under command `0x04`, so a mode change is always possible there.
    pub fn can_change_mode(&self) -> bool {
        match self {
            Keyboard::Wired(k) => k.can_change_mode(),
            // The receiver carries the same config block under command 0x04.
            Keyboard::Wireless(_) => true,
        }
    }
}

impl RgbDevice for Keyboard {
    fn name(&self) -> &str {
        match self {
            Keyboard::Wired(k) => k.name(),
            Keyboard::Wireless(k) => k.name(),
        }
    }

    fn led_count(&self) -> usize {
        match self {
            Keyboard::Wired(k) => k.led_count(),
            Keyboard::Wireless(k) => k.led_count(),
        }
    }

    fn layout(&self) -> &[KeyPos] {
        match self {
            Keyboard::Wired(k) => k.layout(),
            Keyboard::Wireless(k) => k.layout(),
        }
    }

    fn max_fps(&self) -> u32 {
        match self {
            Keyboard::Wired(k) => k.max_fps(),
            Keyboard::Wireless(k) => k.max_fps(),
        }
    }

    fn ensure_per_key_mode(&mut self) -> Result<bool> {
        match self {
            Keyboard::Wired(k) => k.ensure_per_key_mode(),
            Keyboard::Wireless(k) => k.ensure_per_key_mode(),
        }
    }

    fn set_static(&mut self, frame: &Frame) -> Result<()> {
        match self {
            Keyboard::Wired(k) => k.set_static(frame),
            Keyboard::Wireless(k) => k.set_static(frame),
        }
    }

    fn stream(&mut self, frame: &Frame) -> Result<()> {
        match self {
            Keyboard::Wired(k) => k.stream(frame),
            Keyboard::Wireless(k) => k.stream(frame),
        }
    }

    fn set_max_fps(&mut self, fps: u32) {
        match self {
            Keyboard::Wired(k) => k.set_max_fps(fps),
            Keyboard::Wireless(k) => k.set_max_fps(fps),
        }
    }
}
