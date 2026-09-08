//! HID feature-report transport.

use hidapi::{HidApi, HidDevice};

use crate::{Error, Result};

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
        let api = HidApi::new().map_err(Error::Hid)?;

        let candidates: Vec<_> = api
            .device_list()
            .filter(|d| d.vendor_id() == vid && d.product_id() == pid)
            .collect();

        if candidates.is_empty() {
            return Err(Error::NotFound { vid, pid });
        }

        // Vendor-range collections only; the rest are OS-owned.
        let vendor: Vec<_> = candidates
            .iter()
            .filter(|d| d.usage_page() >= 0xff00)
            .collect();

        let mut last_err = None;
        for info in &vendor {
            let dev = match info.open_device(&api) {
                Ok(d) => d,
                Err(e) => {
                    last_err = Some(e);
                    continue;
                }
            };
            let mut buf = vec![0u8; packet_len];
            buf[0] = report_id;
            if dev.get_feature_report(&mut buf).is_ok() {
                return Ok(Self {
                    dev,
                    packet_len,
                    report_id,
                    path: info.path().to_string_lossy().into_owned(),
                    product: info.product_string().unwrap_or_default().to_string(),
                });
            }
        }

        Err(Error::NoRgbInterface {
            vendor_collections: vendor.len(),
            last: last_err.map(|e| e.to_string()),
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

    /// Read one feature report of the protocol's packet length.
    pub fn receive(&self) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; self.packet_len];
        buf[0] = self.report_id;
        let n = self.dev.get_feature_report(&mut buf).map_err(Error::Hid)?;
        buf.truncate(n.max(1));
        Ok(buf)
    }
}

/// List every HID collection matching a VID/PID, for diagnostics.
pub fn list_collections(vid: u16, pid: u16) -> Result<Vec<CollectionInfo>> {
    let api = HidApi::new().map_err(Error::Hid)?;
    Ok(api
        .device_list()
        .filter(|d| d.vendor_id() == vid && d.product_id() == pid)
        .map(|d| CollectionInfo {
            path: d.path().to_string_lossy().into_owned(),
            usage_page: d.usage_page(),
            usage: d.usage(),
            interface: d.interface_number(),
            product: d.product_string().unwrap_or_default().to_string(),
        })
        .collect())
}

#[derive(Clone, Debug)]
pub struct CollectionInfo {
    pub path: String,
    pub usage_page: u16,
    pub usage: u16,
    pub interface: i32,
    pub product: String,
}
