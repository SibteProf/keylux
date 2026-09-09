//! HID feature-report transport, and the device discovery built on top of it.

use std::collections::BTreeMap;
use std::ffi::CString;

use hidapi::{DeviceInfo, HidApi, HidDevice};

use crate::device::{DeviceId, Link};
use crate::{Error, Result};

/// HID usage pages the operating system claims for itself.
///
/// On Windows these top-level collections are opened exclusively for the OS, so
/// a handle to one is useless even though it shares the device's VID/PID.
/// Discovery must never try to open them.
const USAGE_PAGE_GENERIC_DESKTOP: u16 = 0x0001;
const USAGE_PAGE_CONSUMER: u16 = 0x000c;
const USAGE_KEYBOARD: u16 = 0x0006;
/// Vendor-defined usage pages start here.
const USAGE_PAGE_VENDOR_MIN: u16 = 0xff00;

/// A HID connection to one vendor collection.
pub struct Transport {
    dev: HidDevice,
    packet_len: usize,
    report_id: u8,
    pub path: String,
    pub product: String,
}

impl Transport {
    /// Open the vendor collection that actually speaks the RGB protocol.
    ///
    /// Selection is by PROBE, not by usage page. On this hardware:
    ///
    /// * Windows opens keyboard and mouse top-level collections exclusively for
    ///   the OS, so those handles are unusable even though they share the
    ///   VID/PID.
    /// * There are three vendor collections and Windows reports usage page
    ///   0xFF00 for all of them. Published notes cite 0xFF13, and the collection
    ///   index differs between machines — so neither is a reliable selector.
    ///
    /// Only the real RGB collection accepts a full-length feature report on the
    /// protocol's report ID, so that is the test. It is read-only: no write
    /// command is sent.
    pub fn open(vid: u16, pid: u16, report_id: u8, packet_len: usize) -> Result<Self> {
        let id = DeviceId::new(vid, pid);
        let api = HidApi::new().map_err(Error::Hid)?;
        let all: Vec<&DeviceInfo> = api.device_list().collect();

        if !all.iter().any(|d| id_of(d) == id) {
            return Err(Error::NotFound { searched: 1 });
        }

        let mut last_err = None;
        let cols = openable_collections(&all, id, true);
        for info in &cols {
            match open_probed(&api, info, report_id, packet_len) {
                Probe::Answered(t) => return Ok(t),
                Probe::Silent => {}
                Probe::Failed(e) => last_err = Some(e),
            }
        }

        Err(Error::NoRgbInterface {
            vendor_collections: cols.len(),
            last: last_err,
        })
    }

    /// Open one collection by HID path, without enumerating or probing.
    ///
    /// The path must come from a [`DeviceCandidate`] produced in this process.
    /// HID paths are not stable across replugs and differ between USB ports, so
    /// they are never persisted — settings store a [`DeviceId`] instead.
    pub fn open_path(path: &str, report_id: u8, packet_len: usize) -> Result<Self> {
        let api = HidApi::new().map_err(Error::Hid)?;
        let c = CString::new(path).map_err(|_| Error::PathGone {
            path: path.to_string(),
        })?;
        let dev = api.open_path(&c).map_err(|_| Error::PathGone {
            path: path.to_string(),
        })?;
        let product = dev.get_product_string().ok().flatten().unwrap_or_default();
        Ok(Self {
            dev,
            packet_len,
            report_id,
            path: path.to_string(),
            product,
        })
    }

    pub fn packet_len(&self) -> usize {
        self.packet_len
    }

    /// Send one feature report. `buf[0]` must already be the report ID.
    pub fn send(&self, buf: &[u8]) -> Result<()> {
        debug_assert_eq!(buf.len(), self.packet_len, "packet must be full length");
        self.dev.send_feature_report(buf).map_err(Error::Hid)
    }

    /// Send one HID OUTPUT report. `buf[0]` must already be the report ID.
    ///
    /// The 2.4 GHz receiver's lighting channel has no feature reports at all —
    /// a feature write there returns `Incorrect function`. It takes output
    /// reports, which is also why no feature-report probe can ever find it.
    pub fn send_output(&self, buf: &[u8]) -> Result<()> {
        self.dev.write(buf).map_err(Error::Hid).map(|_| ())
    }

    /// Read one feature report of the protocol's packet length.
    pub fn receive(&self) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; self.packet_len];
        buf[0] = self.report_id;
        let n = self.dev.get_feature_report(&mut buf).map_err(Error::Hid)?;
        buf.truncate(n.max(1));
        Ok(buf)
    }
}

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

/// How much a candidate is trusted.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Confidence {
    /// In the built-in table or the caller's allow-list.
    Known,
    /// Turned up by a deep scan of hardware nothing recognises.
    Probed,
}

/// One device that answered the protocol probe.
#[derive(Clone, Debug)]
pub struct DeviceCandidate {
    pub id: DeviceId,
    pub link: Link,
    pub model: &'static str,
    /// HID path of the collection that answered. Valid only for the lifetime of
    /// this enumeration — never persist it.
    pub path: String,
    pub product: String,
    pub confidence: Confidence,
    /// The device returned a config block with the protocol's signature. This
    /// is the difference between "some vendor collection accepted a feature
    /// report" and "this is an AULA board".
    pub confirmed: bool,
}

impl DeviceCandidate {
    /// What the UI, the tray tooltip and the CLI call this device.
    pub fn label(&self) -> String {
        match (self.confidence, self.confirmed) {
            (Confidence::Known, true) => format!("{} ({})", self.model, self.link.suffix()),
            // Known hardware that answered the report but not the config read.
            // Worth showing rather than hiding: it is the shape of a receiver
            // whose control channel is not the keyboard's.
            (Confidence::Known, false) => {
                format!("{} ({}, not answering)", self.model, self.link.suffix())
            }
            (Confidence::Probed, true) => {
                format!("{} ({}, unverified)", self.model, self.link.suffix())
            }
            (Confidence::Probed, false) => {
                let name = if self.product.is_empty() {
                    "unknown device"
                } else {
                    self.product.as_str()
                };
                format!("{name} ({}, unverified)", self.id)
            }
        }
    }
}

/// How wide to cast the net.
#[derive(Clone, Debug, Default)]
pub struct ScanOptions {
    /// Extra ids to treat exactly like the built-in known table, from settings.
    pub allow: Vec<DeviceId>,
    /// Also probe hardware nothing recognises. User-initiated only: opening a
    /// vendor collection can take the handle away from another vendor's utility,
    /// so it must never happen on a timer.
    pub deep: bool,
    /// With `deep`, drop the requirement that the device also present a
    /// keyboard collection.
    pub include_non_keyboards: bool,
}

/// What a scan is looking for.
pub struct ScanSpec<'a> {
    pub model: &'static str,
    pub report_id: u8,
    pub packet_len: usize,
    /// Built-in known devices and the link each one implies.
    pub known: &'a [(DeviceId, Link)],
    pub opts: &'a ScanOptions,
}

/// Find every device that speaks the protocol.
///
/// `confirm` is the real test, and the only one that works: it asks the device
/// something and checks the answer. Accepting a feature report is far too weak
/// — a receiver's status collection does that too, and so does the keyboard
/// itself before any command has been sent. Only devices that confirm are
/// returned; `probe_with` keeps the near-misses for diagnostics.
///
/// It is a callback so this module never has to know a command byte, and
/// whatever it does must be READ-ONLY — `docs/PROTOCOL.md` explains why
/// identifying hardware by trying write commands could brick it.
pub fn discover_with<F>(spec: &ScanSpec<'_>, confirm: F) -> Result<Vec<DeviceCandidate>>
where
    F: Fn(&Transport) -> bool,
{
    let api = HidApi::new().map_err(Error::Hid)?;
    let all: Vec<&DeviceInfo> = api.device_list().collect();

    let mut out = Vec::new();
    for (id, link, confidence) in targets(&all, spec) {
        // At most one candidate per device: the first collection that answers
        // is the RGB one, exactly as the single-device path has always worked.
        let trusted = confidence == Confidence::Known;
        for info in openable_collections(&all, id, trusted) {
            if let Probe::Answered(t) = open_probed(&api, info, spec.report_id, spec.packet_len) {
                if !confirm(&t) {
                    // Answered, but it is not one of ours. Keep looking: the
                    // next collection on this device may be.
                    continue;
                }
                out.push(DeviceCandidate {
                    id,
                    link,
                    model: spec.model,
                    path: t.path.clone(),
                    product: t.product.clone(),
                    confidence,
                    confirmed: true,
                });
                break;
            }
        }
    }
    Ok(out)
}

/// What happened when one collection was probed.
#[derive(Clone, Debug, PartialEq)]
pub enum ProbeResult {
    /// Accepted a full-length feature report on the protocol's report ID.
    Answered {
        /// Whether it also returned data the protocol layer recognises.
        confirmed: bool,
    },
    /// Opened, but would not accept the protocol's report ID at all.
    Silent,
    /// A top-level collection the OS keeps for itself.
    OsOwned,
    /// Could not be opened. On Linux this usually means a missing udev rule.
    Unopenable(String),
}

/// One collection and what probing it revealed. Diagnostics only.
#[derive(Clone, Debug)]
pub struct CollectionProbe {
    pub info: CollectionInfo,
    pub result: ProbeResult,
}

/// Probe every collection a scan would consider, keeping the near-misses.
///
/// `discover_with` throws away everything that is not a usable candidate, which
/// is right for the app and useless for working out *why* a receiver was not
/// found. This keeps the whole picture for the `scan` command.
pub fn probe_with<F>(spec: &ScanSpec<'_>, confirm: F) -> Result<Vec<CollectionProbe>>
where
    F: Fn(&Transport) -> bool,
{
    let api = HidApi::new().map_err(Error::Hid)?;
    let all: Vec<&DeviceInfo> = api.device_list().collect();

    let mut out = Vec::new();
    for (id, _, confidence) in targets(&all, spec) {
        let trusted = confidence == Confidence::Known;
        let openable = openable_collections(&all, id, trusted);
        for info in collections(&all, id) {
            let result = if !openable.iter().any(|o| o.path() == info.path()) {
                ProbeResult::OsOwned
            } else {
                match open_probed(&api, info, spec.report_id, spec.packet_len) {
                    Probe::Answered(t) => ProbeResult::Answered {
                        confirmed: confirm(&t),
                    },
                    Probe::Silent => ProbeResult::Silent,
                    Probe::Failed(e) => ProbeResult::Unopenable(e),
                }
            };
            out.push(CollectionProbe {
                info: describe(info),
                result,
            });
        }
    }
    Ok(out)
}

/// Which devices this scan should look at, and what we already believe of them.
fn targets(all: &[&DeviceInfo], spec: &ScanSpec<'_>) -> Vec<(DeviceId, Link, Confidence)> {
    // BTreeMap so the order is stable across runs; a picker that reshuffles
    // itself between scans is unusable.
    let mut found: BTreeMap<(u16, u16), (DeviceId, Link, Confidence)> = BTreeMap::new();

    for d in all {
        let id = id_of(d);
        if let Some((_, link)) = spec.known.iter().find(|(k, _)| *k == id) {
            found
                .entry((id.vid, id.pid))
                .or_insert((id, *link, Confidence::Known));
        } else if spec.opts.allow.contains(&id) {
            // The user allow-listed this after a scan found it. The only reason
            // Deliberately NOT Link::Dongle: that now selects the receiver's
            // own protocol. Something reached here by confirming the wired
            // config block is speaking the wired protocol, whatever it is
            // plugged into.
            found
                .entry((id.vid, id.pid))
                .or_insert((id, Link::Unknown, Confidence::Known));
        }
    }

    if spec.opts.deep {
        for d in all {
            let id = id_of(d);
            if found.contains_key(&(id.vid, id.pid)) {
                continue;
            }
            if !spec.opts.include_non_keyboards && !is_keyboard_shaped(all, id) {
                continue;
            }
            if collections(all, id).any(is_vendor) {
                found
                    .entry((id.vid, id.pid))
                    .or_insert((id, Link::Unknown, Confidence::Probed));
            }
        }
    }

    found.into_values().collect()
}

/// Does this device present a keyboard, as a receiver for one would?
///
/// Cheap, and it keeps a deep scan away from fingerprint readers, webcams and
/// anything else that merely happens to have a vendor collection.
fn is_keyboard_shaped(all: &[&DeviceInfo], id: DeviceId) -> bool {
    collections(all, id)
        .any(|d| d.usage_page() == USAGE_PAGE_GENERIC_DESKTOP && d.usage() == USAGE_KEYBOARD)
}

fn collections<'a>(
    all: &'a [&'a DeviceInfo],
    id: DeviceId,
) -> impl Iterator<Item = &'a DeviceInfo> + 'a {
    all.iter().copied().filter(move |d| id_of(d) == id)
}

fn is_vendor(d: &DeviceInfo) -> bool {
    d.usage_page() >= USAGE_PAGE_VENDOR_MIN
}

/// The collections worth trying to open on one device.
///
/// `trusted` relaxes the filter for hardware we already believe is ours. The
/// hidraw backend on Linux has historically reported a usage page only for the
/// first top-level collection, which would leave the vendor-range filter
/// matching nothing at all. Falling back to "anything the OS has not claimed"
/// recovers those, and is only safe on a device we already recognise — a deep
/// scan of unknown hardware keeps the strict filter.
fn openable_collections<'a>(
    all: &'a [&'a DeviceInfo],
    id: DeviceId,
    trusted: bool,
) -> Vec<&'a DeviceInfo> {
    let vendor: Vec<_> = collections(all, id).filter(|d| is_vendor(d)).collect();
    if !vendor.is_empty() || !trusted {
        return vendor;
    }
    collections(all, id)
        .filter(|d| {
            d.usage_page() != USAGE_PAGE_GENERIC_DESKTOP && d.usage_page() != USAGE_PAGE_CONSUMER
        })
        .collect()
}

enum Probe {
    Answered(Transport),
    /// Opened fine, but did not accept the protocol's feature report.
    Silent,
    /// Could not be opened at all — on Linux this usually means a missing udev
    /// rule rather than the wrong collection.
    Failed(String),
}

fn open_probed(api: &HidApi, info: &DeviceInfo, report_id: u8, packet_len: usize) -> Probe {
    let dev = match info.open_device(api) {
        Ok(d) => d,
        Err(e) => return Probe::Failed(e.to_string()),
    };
    let mut buf = vec![0u8; packet_len];
    buf[0] = report_id;
    // Length is deliberately not checked. The real keyboard answers a bare
    // read — one sent before any command — with 9 bytes, the same as the
    // receiver's status collection, so width cannot tell them apart. Only the
    // reply to a config read can, and that is the caller's `confirm`.
    if dev.get_feature_report(&mut buf).is_err() {
        return Probe::Silent;
    }

    Probe::Answered(Transport {
        dev,
        packet_len,
        report_id,
        path: info.path().to_string_lossy().into_owned(),
        product: info.product_string().unwrap_or_default().to_string(),
    })
}

// ---------------------------------------------------------------------------
// Choosing between candidates
// ---------------------------------------------------------------------------

/// The outcome of picking a device to drive.
#[derive(Clone, Debug, PartialEq)]
pub enum Choice {
    /// Index into the candidate list.
    Use(usize),
    /// The user pinned this device and it is not here.
    PinnedMissing(DeviceId),
    None,
}

/// Decide which candidate to drive.
///
/// A pin is honoured or reported missing — NEVER silently substituted. Someone
/// who pinned the receiver and then unplugged it must not find the app quietly
/// lighting the wired board instead; that is the confusion the picker exists to
/// prevent, and the UI offers a one-click way back.
///
/// With no pin: wired first (it is the measured, faster path), then a receiver,
/// and a confirmed device beats an unconfirmed one either way.
pub fn choose(candidates: &[DeviceCandidate], pinned: Option<DeviceId>) -> Choice {
    if let Some(id) = pinned {
        return match candidates.iter().position(|c| c.id == id) {
            Some(i) => Choice::Use(i),
            None => Choice::PinnedMissing(id),
        };
    }
    let rank = |c: &DeviceCandidate| {
        let link = match c.link {
            Link::Wired => 0,
            Link::Dongle => 1,
            Link::Unknown => 2,
        };
        (link, u8::from(!c.confirmed))
    };
    candidates
        .iter()
        .enumerate()
        .min_by_key(|(_, c)| rank(c))
        .map(|(i, _)| Choice::Use(i))
        .unwrap_or(Choice::None)
}

// ---------------------------------------------------------------------------
// Diagnostics
// ---------------------------------------------------------------------------

/// A cheap fingerprint of what is plugged in.
///
/// Enumerating is cheap; probing is not, and probing means opening handles.
/// The engine compares this between retries so it only re-probes when the set
/// of HID devices has actually changed.
pub fn enumeration_signature() -> Result<Vec<String>> {
    let api = HidApi::new().map_err(Error::Hid)?;
    let mut sig: Vec<String> = api
        .device_list()
        .map(|d| d.path().to_string_lossy().into_owned())
        .collect();
    sig.sort();
    Ok(sig)
}

/// Every HID collection on the system, for diagnostics.
pub fn list_all() -> Result<Vec<CollectionInfo>> {
    let api = HidApi::new().map_err(Error::Hid)?;
    Ok(api.device_list().map(describe).collect())
}

/// Every HID collection matching a VID/PID, for diagnostics.
pub fn list_collections(vid: u16, pid: u16) -> Result<Vec<CollectionInfo>> {
    let id = DeviceId::new(vid, pid);
    Ok(list_all()?.into_iter().filter(|c| c.id == id).collect())
}

fn describe(d: &DeviceInfo) -> CollectionInfo {
    CollectionInfo {
        id: id_of(d),
        path: d.path().to_string_lossy().into_owned(),
        usage_page: d.usage_page(),
        usage: d.usage(),
        interface: d.interface_number(),
        product: d.product_string().unwrap_or_default().to_string(),
        manufacturer: d.manufacturer_string().unwrap_or_default().to_string(),
    }
}

fn id_of(d: &DeviceInfo) -> DeviceId {
    DeviceId::new(d.vendor_id(), d.product_id())
}

#[derive(Clone, Debug)]
pub struct CollectionInfo {
    pub id: DeviceId,
    pub path: String,
    pub usage_page: u16,
    pub usage: u16,
    pub interface: i32,
    pub product: String,
    pub manufacturer: String,
}

impl CollectionInfo {
    /// Would discovery try to open this collection?
    pub fn is_vendor(&self) -> bool {
        self.usage_page >= USAGE_PAGE_VENDOR_MIN
    }

    /// Is this one of the collections the OS keeps for itself?
    pub fn is_os_owned(&self) -> bool {
        self.usage_page == USAGE_PAGE_GENERIC_DESKTOP || self.usage_page == USAGE_PAGE_CONSUMER
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(id: DeviceId, link: Link, confirmed: bool) -> DeviceCandidate {
        DeviceCandidate {
            id,
            link,
            model: "AULA F75",
            path: format!("path-{id}"),
            product: "Gaming Keyboard".into(),
            confidence: Confidence::Known,
            confirmed,
        }
    }

    const WIRED: DeviceId = DeviceId::new(0x258a, 0x010c);
    const DONGLE: DeviceId = DeviceId::new(0x3554, 0xfa09);

    #[test]
    fn wired_wins_when_nothing_is_pinned() {
        let c = vec![
            candidate(DONGLE, Link::Dongle, true),
            candidate(WIRED, Link::Wired, true),
        ];
        assert_eq!(choose(&c, None), Choice::Use(1));
    }

    #[test]
    fn a_pin_beats_the_wired_preference() {
        let c = vec![
            candidate(WIRED, Link::Wired, true),
            candidate(DONGLE, Link::Dongle, true),
        ];
        assert_eq!(choose(&c, Some(DONGLE)), Choice::Use(1));
    }

    /// The rule that keeps the picker honest: pinning the receiver and
    /// unplugging it must not hand the lighting back to the cable.
    #[test]
    fn a_missing_pin_never_falls_back_to_a_present_device() {
        let c = vec![candidate(WIRED, Link::Wired, true)];
        assert_eq!(choose(&c, Some(DONGLE)), Choice::PinnedMissing(DONGLE));
    }

    #[test]
    fn nothing_plugged_in_is_not_a_choice() {
        assert_eq!(choose(&[], None), Choice::None);
        assert_eq!(choose(&[], Some(WIRED)), Choice::PinnedMissing(WIRED));
    }

    #[test]
    fn a_confirmed_device_breaks_a_tie() {
        let c = vec![
            candidate(DONGLE, Link::Dongle, false),
            candidate(DeviceId::new(0x1d57, 0xfa60), Link::Dongle, true),
        ];
        assert_eq!(choose(&c, None), Choice::Use(1));
    }

    #[test]
    fn labels_say_how_the_board_is_attached() {
        let mut c = candidate(WIRED, Link::Wired, true);
        assert_eq!(c.label(), "AULA F75 (wired)");

        c.link = Link::Dongle;
        assert_eq!(c.label(), "AULA F75 (2.4 GHz dongle)");

        c.confidence = Confidence::Probed;
        assert_eq!(c.label(), "AULA F75 (2.4 GHz dongle, unverified)");

        // Nothing has confirmed it is even an AULA board, so it is named by
        // what the device itself reports rather than by our model name.
        c.confirmed = false;
        c.id = DeviceId::new(0x3554, 0xfa09);
        assert_eq!(c.label(), "Gaming Keyboard (3554:fa09, unverified)");
    }
}
