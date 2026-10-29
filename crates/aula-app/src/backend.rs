//! Selectable native AULA and OpenRGB backends shared by the GUI and CLI.

use std::net::SocketAddr;

use aula_protocol::openrgb::{Client, Controller, OpenRgbKeyboard};
use aula_protocol::{
    ChannelOrder, DeviceCandidate, Frame, KeyPos, Keyboard, Link, RgbDevice, ScanOptions,
};

#[derive(Clone, Debug)]
pub enum Candidate {
    Native(DeviceCandidate),
    OpenRgb(Controller),
}

impl Candidate {
    pub fn id(&self) -> String {
        match self {
            Self::Native(c) => c.id.to_string(),
            Self::OpenRgb(c) => c.key(),
        }
    }
    pub fn label(&self) -> String {
        match self {
            Self::Native(c) => c.label(),
            Self::OpenRgb(c) => format!(
                "{} (OpenRGB · {})",
                c.name,
                if c.serial.is_empty() {
                    &c.location
                } else {
                    &c.serial
                }
            ),
        }
    }
    pub fn link(&self) -> Link {
        match self {
            Self::Native(c) => c.link,
            Self::OpenRgb(_) => Link::OpenRgb,
        }
    }
    pub fn confirmed(&self) -> bool {
        match self {
            Self::Native(c) => c.confirmed,
            Self::OpenRgb(_) => true,
        }
    }
}

pub struct Discovery {
    pub candidates: Vec<Candidate>,
    pub warnings: Vec<String>,
}

pub fn discover(opts: &ScanOptions, endpoint: Option<SocketAddr>) -> Discovery {
    DiscoveryCache::default().scan(opts, endpoint, true)
}

pub fn discover_openrgb(endpoint: SocketAddr) -> Discovery {
    let mut found = Discovery {
        candidates: Vec::new(),
        warnings: Vec::new(),
    };
    append_openrgb(&mut found.candidates, &mut found.warnings, Some(endpoint));
    found
}

/// A server restart need not repeatedly open the native HID collections.
#[derive(Default)]
pub struct DiscoveryCache {
    signature: Option<Vec<String>>,
    native: Vec<DeviceCandidate>,
    native_warning: Option<String>,
}

impl DiscoveryCache {
    pub fn scan(
        &mut self,
        opts: &ScanOptions,
        endpoint: Option<SocketAddr>,
        force: bool,
    ) -> Discovery {
        let signature = aula_protocol::transport::enumeration_signature().ok();
        if force || signature.is_none() || signature != self.signature {
            self.signature = signature;
            match Keyboard::discover(opts) {
                Ok(found) => {
                    self.native = found;
                    self.native_warning = None;
                }
                Err(e) => {
                    self.native.clear();
                    self.native_warning = Some(e.to_string());
                }
            }
        }
        let mut candidates: Vec<_> = self.native.iter().cloned().map(Candidate::Native).collect();
        let mut warnings: Vec<_> = self.native_warning.iter().cloned().collect();
        append_openrgb(&mut candidates, &mut warnings, endpoint);
        candidates.sort_by_key(rank);
        Discovery {
            candidates,
            warnings,
        }
    }
}

fn append_openrgb(
    candidates: &mut Vec<Candidate>,
    warnings: &mut Vec<String>,
    endpoint: Option<SocketAddr>,
) {
    if let Some(endpoint) = endpoint {
        match Client::connect(endpoint).and_then(|mut c| c.controllers()) {
            Ok(found) => {
                for c in found.into_iter().filter(Controller::is_keyboard) {
                    if c.supports_direct() && c.led_count > 0 {
                        candidates.push(Candidate::OpenRgb(c));
                    } else {
                        warnings.push(format!(
                            "{}: OpenRGB does not expose per-key Direct mode",
                            c.name
                        ));
                    }
                }
            }
            Err(e) => warnings.push(e.to_string()),
        }
    }
}

fn rank(c: &Candidate) -> (u8, bool) {
    (
        match c.link() {
            Link::Wired => 0,
            Link::Dongle => 1,
            Link::Unknown => 2,
            Link::OpenRgb => 3,
        },
        !c.confirmed(),
    )
}

/// A missing pin never silently changes the keyboard that gets written.
pub fn choose(candidates: &[Candidate], pinned: Option<&str>) -> Option<usize> {
    match pinned {
        Some(id) => candidates.iter().position(|c| c.id() == id),
        None => candidates
            .iter()
            .enumerate()
            .min_by_key(|(_, c)| rank(c))
            .map(|(i, _)| i),
    }
}

pub enum Device {
    Native(Keyboard),
    OpenRgb(OpenRgbKeyboard),
}

impl Device {
    pub fn open(c: &Candidate, endpoint: Option<SocketAddr>) -> anyhow::Result<Self> {
        Ok(match c {
            Candidate::Native(c) => Self::Native(Keyboard::open_candidate(c, ChannelOrder::Rgb)?),
            Candidate::OpenRgb(c) => Self::OpenRgb(OpenRgbKeyboard::open(
                endpoint.ok_or_else(|| anyhow::anyhow!("OpenRGB is disabled"))?,
                &c.key(),
            )?),
        })
    }
    fn inner(&self) -> &dyn RgbDevice {
        match self {
            Self::Native(k) => k,
            Self::OpenRgb(k) => k,
        }
    }
    fn inner_mut(&mut self) -> &mut dyn RgbDevice {
        match self {
            Self::Native(k) => k,
            Self::OpenRgb(k) => k,
        }
    }
    pub fn id(&self) -> String {
        match self {
            Self::Native(k) => k.id().to_string(),
            Self::OpenRgb(k) => k.info().key(),
        }
    }
    pub fn link(&self) -> Link {
        match self {
            Self::Native(k) => k.link(),
            Self::OpenRgb(_) => Link::OpenRgb,
        }
    }
    pub fn hid_path(&self) -> &str {
        match self {
            Self::Native(k) => k.hid_path(),
            Self::OpenRgb(k) => k.endpoint(),
        }
    }
    pub fn can_change_mode(&self) -> bool {
        match self {
            Self::Native(k) => k.can_change_mode(),
            Self::OpenRgb(_) => true,
        }
    }
    pub fn can_save(&self) -> bool {
        matches!(self, Self::Native(_))
    }
    pub fn has_matrix(&self) -> bool {
        match self {
            Self::Native(_) => true,
            Self::OpenRgb(k) => k.info().has_matrix,
        }
    }
    pub fn fps_ceiling(&self) -> u32 {
        match self {
            Self::Native(k) => k.max_fps(),
            Self::OpenRgb(_) => aula_protocol::openrgb::MAX_FPS,
        }
    }
}

impl RgbDevice for Device {
    fn name(&self) -> &str {
        self.inner().name()
    }
    fn led_count(&self) -> usize {
        self.inner().led_count()
    }
    fn layout(&self) -> &[KeyPos] {
        self.inner().layout()
    }
    fn max_fps(&self) -> u32 {
        self.inner().max_fps()
    }
    fn set_max_fps(&mut self, fps: u32) {
        self.inner_mut().set_max_fps(fps);
    }
    fn ensure_per_key_mode(&mut self) -> aula_protocol::Result<bool> {
        self.inner_mut().ensure_per_key_mode()
    }
    fn set_static(&mut self, frame: &Frame) -> aula_protocol::Result<()> {
        self.inner_mut().set_static(frame)
    }
    fn stream(&mut self, frame: &Frame) -> aula_protocol::Result<()> {
        self.inner_mut().stream(frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn native(id: &str, link: Link, confirmed: bool) -> Candidate {
        Candidate::Native(DeviceCandidate {
            model: "Test keyboard",
            id: id.parse().unwrap(),
            link,
            confirmed,
            path: "test-path".into(),
            product: "Test keyboard".into(),
            confidence: aula_protocol::Confidence::Known,
        })
    }

    #[test]
    fn selection_preserves_native_priority_and_never_substitutes_a_missing_pin() {
        let candidates = vec![
            native("258a:010c", Link::Wired, false),
            native("3554:fa09", Link::Dongle, true),
            native("258a:010d", Link::Wired, true),
        ];
        assert_eq!(choose(&candidates, None), Some(2));
        assert_eq!(choose(&candidates, Some("3554:fa09")), Some(1));
        assert_eq!(choose(&candidates, Some("openrgb:missing")), None);
        assert_eq!(choose(&[], None), None);
    }
}
