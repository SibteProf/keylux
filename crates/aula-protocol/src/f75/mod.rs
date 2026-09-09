//! AULA F75 driver.

pub mod keymap;
pub mod protocol;

use std::thread;
use std::time::{Duration, Instant};

use crate::color::ChannelOrder;
use crate::device::{DeviceId, Frame, KeyPos, Link, RgbDevice};
use crate::transport::{
    self, Choice, Confidence, DeviceCandidate, ScanOptions, ScanSpec, Transport,
};
use crate::{Error, Result};

use protocol as p;

pub const MODEL: &str = "AULA F75";

pub struct F75 {
    io: Transport,
    layout: Vec<KeyPos>,
    order: ChannelOrder,
    /// Enforces the minimum gap between colour writes.
    last_write: Option<Instant>,
    min_gap: Duration,
    /// Last frame actually sent, so identical ones can be skipped.
    last_frame: Option<Frame>,
    id: DeviceId,
    link: Link,
    /// Owned rather than a `&'static str`, because the name now says how the
    /// board is attached and that is only known at open time.
    name: String,
    /// Whether a config write is allowed. False for a device a scan turned up
    /// but nothing has confirmed: streaming to it is volatile and harmless,
    /// where a config write is neither.
    allow_config_write: bool,
}

/// The gap to use for a requested frame rate on a link.
///
/// Split out as a free function so the clamping rule — which is what stands
/// between a user's FPS slider and a keyboard that drops keypresses — can be
/// tested without hardware.
pub(crate) fn clamped_gap(requested_fps: u32, link: Link) -> Duration {
    let floor = p::min_frame_gap_ms(link);
    // Only ever slower. The hardware ceiling is not a preference, and a caller
    // asking for more than the firmware can absorb is asking for dropped
    // keypresses.
    let fps = requested_fps.clamp(1, p::max_fps_for(link));
    let gap = 1000 / u64::from(fps);
    Duration::from_millis(gap.max(floor))
}

/// What a scan for this model looks for.
fn scan_spec<'a>(opts: &'a ScanOptions) -> ScanSpec<'a> {
    ScanSpec {
        model: MODEL,
        report_id: p::REPORT_ID,
        packet_len: p::PACKET_LEN,
        known: p::KNOWN,
        opts,
    }
}

/// Confirm a probed collection really is an AULA board.
///
/// Reads the config block and checks the `5A A5` signature. `0x84` is a read
/// command the vendor driver itself issues, so this stays inside the safety
/// rule in `docs/PROTOCOL.md`: identify hardware by reading, never by trying
/// write commands. "Accepted a feature report" on its own is a weak signal
/// across a bus full of vendor collections; a valid config block is not.
///
/// This RETRIES, for the same reason [`F75::read_config`] does: the firmware
/// returns truncated, mostly-zero buffers while it is busy, and a board that
/// has only just been plugged in is exactly when this runs. A single-shot read
/// labels a perfectly good wired keyboard "not answering".
fn speaks_the_protocol(io: &Transport) -> bool {
    const ATTEMPTS: usize = 4;
    for attempt in 0..ATTEMPTS {
        if attempt > 0 {
            thread::sleep(Duration::from_millis(30));
        }
        if io
            .send(&p::build_packet(
                p::cmd::READ_CONFIG,
                p::ADDR,
                p::CONFIG_LEN as u16,
                None,
            ))
            .is_err()
        {
            // A collection that will not even take the command is not ours,
            // and waiting will not change that.
            return false;
        }
        if let Ok(resp) = io.receive() {
            if resp.len() >= p::HEADER_LEN + p::CONFIG_LEN
                && p::is_valid_config(&resp[p::HEADER_LEN..p::HEADER_LEN + p::CONFIG_LEN])
            {
                return true;
            }
        }
    }
    false
}

impl F75 {
    /// Open the best available board: wired first, then a receiver.
    pub fn open() -> Result<Self> {
        Self::open_with(ChannelOrder::Rgb)
    }

    pub fn open_with(order: ChannelOrder) -> Result<Self> {
        Self::open_best(&ScanOptions::default(), order)
    }

    /// Open the best candidate a scan with these options turns up.
    pub fn open_best(opts: &ScanOptions, order: ChannelOrder) -> Result<Self> {
        let found = Self::discover(opts)?;
        match transport::choose(&found, None) {
            Choice::Use(i) => Self::open_candidate(&found[i], order),
            _ => Err(Error::NotFound {
                searched: p::KNOWN.len() + opts.allow.len(),
            }),
        }
    }

    /// Every device that answers this protocol.
    ///
    /// Read-only. With the default options it looks only at known and
    /// allow-listed ids; `opts.deep` widens it to unrecognised hardware and is
    /// for user-initiated scans only.
    pub fn discover(opts: &ScanOptions) -> Result<Vec<DeviceCandidate>> {
        transport::discover_with(&scan_spec(opts), speaks_the_protocol)
    }

    /// The same scan, keeping every near-miss. For diagnostics.
    pub fn probe(opts: &ScanOptions) -> Result<Vec<transport::CollectionProbe>> {
        transport::probe_with(&scan_spec(opts), speaks_the_protocol)
    }

    pub fn open_candidate(c: &DeviceCandidate, order: ChannelOrder) -> Result<Self> {
        let io = Transport::open_path(&c.path, p::REPORT_ID, p::PACKET_LEN)?;
        Ok(Self {
            io,
            layout: keymap::layout(),
            order,
            last_write: None,
            min_gap: Duration::from_millis(p::min_frame_gap_ms(c.link)),
            last_frame: None,
            id: c.id,
            link: c.link,
            name: format!("{MODEL} ({})", c.link.suffix()),
            // A board that returned a valid config block has proven what it is.
            allow_config_write: c.confidence == Confidence::Known || c.confirmed,
        })
    }

    /// Open one specific device, by id.
    pub fn open_id(id: DeviceId, opts: &ScanOptions, order: ChannelOrder) -> Result<Self> {
        let found = Self::discover(opts)?;
        match transport::choose(&found, Some(id)) {
            Choice::Use(i) => Self::open_candidate(&found[i], order),
            _ => Err(Error::NotFound {
                searched: p::KNOWN.len() + opts.allow.len(),
            }),
        }
    }

    pub fn hid_path(&self) -> &str {
        &self.io.path
    }

    pub fn id(&self) -> DeviceId {
        self.id
    }

    pub fn link(&self) -> Link {
        self.link
    }

    /// Whether this board will accept a lighting-mode change.
    pub fn can_change_mode(&self) -> bool {
        self.allow_config_write
    }

    pub fn set_channel_order(&mut self, order: ChannelOrder) {
        self.order = order;
    }

    /// Read the 128-byte config block, rejecting corrupt responses.
    ///
    /// The firmware returns truncated, mostly-zero buffers while it is busy.
    /// Since mode selection is read-modify-write, accepting one and writing it
    /// back would zero the entire config block — so this retries and then fails
    /// loudly rather than returning something unusable.
    pub fn read_config(&self) -> Result<Vec<u8>> {
        const ATTEMPTS: usize = 4;
        let mut last = Vec::new();
        for _ in 0..ATTEMPTS {
            self.io.send(&p::build_packet(
                p::cmd::READ_CONFIG,
                p::ADDR,
                p::CONFIG_LEN as u16,
                None,
            ))?;
            let resp = self.io.receive()?;
            if resp.len() >= p::HEADER_LEN + p::CONFIG_LEN {
                let cfg = resp[p::HEADER_LEN..p::HEADER_LEN + p::CONFIG_LEN].to_vec();
                if p::is_valid_config(&cfg) {
                    return Ok(cfg);
                }
                last = cfg;
            }
            // The radio is slower to answer, so give it proportionally longer
            // before deciding a read was a glitch rather than latency.
            thread::sleep(Duration::from_millis(if self.link == Link::Wired {
                20
            } else {
                40
            }));
        }
        Err(Error::BadConfigRead {
            magic: if last.len() == p::CONFIG_LEN {
                [last[126], last[127]]
            } else {
                [0, 0]
            },
        })
    }

    /// Write the config block.
    ///
    /// Private on purpose. A config write drops the board out of per-key mode
    /// and triggers an async repaint that overwrites in-flight frames — it must
    /// never happen from an animation loop.
    fn write_config(&mut self, cfg: &[u8]) -> Result<()> {
        // The one non-volatile operation in the driver, so it is also the one
        // that must not run on hardware a scan merely found.
        if !self.allow_config_write {
            return Err(Error::UnverifiedDevice { id: self.id });
        }
        if !p::is_valid_config(cfg) {
            return Err(Error::RefusedConfigWrite);
        }
        self.io.send(&p::build_packet(
            p::cmd::WRITE_CONFIG,
            p::ADDR,
            p::CONFIG_LEN as u16,
            Some(cfg),
        ))?;
        // Let the firmware's asynchronous repaint finish before any frame is
        // written, otherwise the repaint lands on top of it.
        thread::sleep(Duration::from_millis(p::config_settle_ms(self.link)));
        self.last_write = Some(Instant::now());
        // The repaint this triggers wipes whatever was on the board, so the
        // next frame must go out even if it matches the cached one.
        self.last_frame = None;
        Ok(())
    }

    /// Is the board currently in per-key mode?
    pub fn is_per_key_mode(&self) -> Result<bool> {
        let cfg = self.read_config()?;
        Ok(cfg[p::mode::OFF_A] == p::mode::PER_KEY[0]
            && cfg[p::mode::OFF_B] == p::mode::PER_KEY[1]
            && cfg[p::mode::OFF_C] == p::mode::PER_KEY[2])
    }

    /// Whether an unchanged frame is due to be resent anyway.
    fn due_for_keepalive(&self) -> bool {
        self.last_write
            .map(|t| t.elapsed() >= Duration::from_millis(p::KEEPALIVE_MS))
            .unwrap_or(true)
    }

    /// Sleep out the remainder of the minimum inter-write gap.
    ///
    /// Writing faster than the firmware can absorb starves its key-scanning
    /// loop and the keyboard stops responding to keypresses until replugged.
    fn respect_gap(&mut self) {
        if let Some(prev) = self.last_write {
            let min = self.min_gap;
            let elapsed = prev.elapsed();
            if elapsed < min {
                thread::sleep(min - elapsed);
            }
        }
        self.last_write = Some(Instant::now());
    }

    /// Encode a frame for the STATIC path: planar, 128 slots.
    fn encode_static(&self, frame: &Frame) -> Vec<u8> {
        let mut buf = vec![0u8; p::STATIC_LEN];
        for i in 0..p::STATIC_SLOTS {
            let [a, b, c] = self.order.apply(frame.get(i));
            buf[i] = a;
            buf[p::STATIC_SLOTS + i] = b;
            buf[2 * p::STATIC_SLOTS + i] = c;
        }
        buf
    }

    /// Encode a frame for the STREAM path: interleaved, 126 slots.
    fn encode_stream(&self, frame: &Frame) -> Vec<u8> {
        let mut buf = vec![0u8; p::STREAM_LEN];
        for i in 0..p::STREAM_SLOTS {
            let [a, b, c] = self.order.apply(frame.get(i));
            buf[i * 3] = a;
            buf[i * 3 + 1] = b;
            buf[i * 3 + 2] = c;
        }
        buf
    }

    /// Read the key matrix table (command 0x83).
    ///
    /// This is how the LED ordering was derived; kept so a future device can be
    /// mapped the same way instead of by hand.
    pub fn read_key_matrix(&self) -> Result<Vec<u8>> {
        self.io.send(&p::build_packet(
            p::cmd::READ_MATRIX,
            [0, 0, 0, 0],
            p::PAYLOAD_LEN as u16,
            None,
        ))?;
        let resp = self.io.receive()?;
        Ok(resp.get(p::HEADER_LEN..).unwrap_or(&[]).to_vec())
    }
}

impl RgbDevice for F75 {
    fn name(&self) -> &str {
        &self.name
    }

    fn led_count(&self) -> usize {
        // The streaming path is the narrower of the two; sizing frames to it
        // means one frame works for both paths.
        p::STREAM_SLOTS
    }

    fn layout(&self) -> &[KeyPos] {
        &self.layout
    }

    fn max_fps(&self) -> u32 {
        p::max_fps_for(self.link)
    }

    fn ensure_per_key_mode(&mut self) -> Result<bool> {
        if self.is_per_key_mode()? {
            return Ok(false);
        }
        // Write the whole known-good block rather than patching bytes: all
        // three mode bytes matter, and with only two of them set the board goes
        // completely dark.
        self.write_config(&p::PER_KEY_CONFIG)?;
        Ok(true)
    }

    fn set_static(&mut self, frame: &Frame) -> Result<()> {
        // Different command and a different payload layout, so what the stream
        // path has cached no longer describes the board.
        self.last_frame = None;
        self.respect_gap();
        let payload = self.encode_static(frame);
        self.io.send(&p::build_packet(
            p::cmd::WRITE_STATIC,
            p::ADDR,
            p::STATIC_LEN as u16,
            Some(&payload),
        ))
    }

    fn stream(&mut self, frame: &Frame) -> Result<()> {
        // Re-sending a frame the board is already showing buys nothing and
        // costs key-scan time, so a still image or a paused effect settles to
        // no traffic at all. The keepalive covers the one case where silence
        // would be wrong: the firmware repainting itself behind our back.
        if !self.due_for_keepalive() && self.last_frame.as_ref() == Some(frame) {
            return Ok(());
        }

        self.respect_gap();
        let payload = self.encode_stream(frame);
        self.io.send(&p::build_packet(
            p::cmd::STREAM,
            p::ADDR,
            p::STREAM_LEN as u16,
            Some(&payload),
        ))?;
        self.last_frame = Some(frame.clone());
        Ok(())
    }

    fn set_max_fps(&mut self, fps: u32) {
        self.min_gap = clamped_gap(fps, self.link);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgb;

    /// Encoding is pure, so it can be tested without hardware.
    fn encode_static_with(order: ChannelOrder, frame: &Frame) -> Vec<u8> {
        let mut buf = vec![0u8; p::STATIC_LEN];
        for i in 0..p::STATIC_SLOTS {
            let [a, b, c] = order.apply(frame.get(i));
            buf[i] = a;
            buf[p::STATIC_SLOTS + i] = b;
            buf[2 * p::STATIC_SLOTS + i] = c;
        }
        buf
    }

    fn encode_stream_with(order: ChannelOrder, frame: &Frame) -> Vec<u8> {
        let mut buf = vec![0u8; p::STREAM_LEN];
        for i in 0..p::STREAM_SLOTS {
            let [a, b, c] = order.apply(frame.get(i));
            buf[i * 3] = a;
            buf[i * 3 + 1] = b;
            buf[i * 3 + 2] = c;
        }
        buf
    }

    #[test]
    fn static_layout_is_planar() {
        let mut f = Frame::black(p::STATIC_SLOTS);
        f.set(0, Rgb::new(255, 0, 0));
        f.set(1, Rgb::new(0, 255, 0));
        let buf = encode_static_with(ChannelOrder::Rgb, &f);

        // red of key 0 at offset 0; green of key 1 in the SECOND plane
        assert_eq!(buf[0], 255);
        assert_eq!(buf[p::STATIC_SLOTS + 1], 255);
        // key 1 has no red, key 0 has no green
        assert_eq!(buf[1], 0);
        assert_eq!(buf[p::STATIC_SLOTS], 0);
    }

    #[test]
    fn stream_layout_is_interleaved() {
        let mut f = Frame::black(p::STREAM_SLOTS);
        f.set(0, Rgb::new(255, 0, 0));
        f.set(1, Rgb::new(0, 255, 0));
        let buf = encode_stream_with(ChannelOrder::Rgb, &f);

        assert_eq!(&buf[0..3], &[255, 0, 0]);
        assert_eq!(&buf[3..6], &[0, 255, 0]);
    }

    #[test]
    fn single_byte_at_offset_one_is_key_one_red() {
        // Mirrors the hardware probe that settled the layout: one 0xFF byte at
        // offset 1 lit the backtick key (LED index 1) red.
        let mut f = Frame::black(p::STATIC_SLOTS);
        f.set(1, Rgb::new(255, 0, 0));
        let buf = encode_static_with(ChannelOrder::Rgb, &f);
        assert_eq!(buf[1], 255);
        assert_eq!(buf.iter().filter(|b| **b != 0).count(), 1);
    }

    #[test]
    fn encoded_lengths_match_protocol() {
        let f = Frame::solid(p::STATIC_SLOTS, Rgb::WHITE);
        assert_eq!(encode_static_with(ChannelOrder::Rgb, &f).len(), 384);
        assert_eq!(encode_stream_with(ChannelOrder::Rgb, &f).len(), 378);
    }

    /// The clamp is what stands between a user's FPS slider and a keyboard that
    /// drops keypresses, so it is tested directly rather than through a device.
    #[test]
    fn a_frame_rate_request_can_only_ever_slow_things_down() {
        for link in [Link::Wired, Link::Dongle, Link::Unknown] {
            let floor = Duration::from_millis(p::min_frame_gap_ms(link));
            // Never faster than the floor. It may land slightly slower: the
            // ceiling is a whole number of frames per second, so 21 FPS on the
            // wired board is a 47 ms gap rather than exactly 46.
            assert!(
                clamped_gap(60, link) >= floor,
                "{link:?} let a fast request through"
            );
            assert!(clamped_gap(u32::MAX, link) >= floor, "{link:?}");
            assert_eq!(
                clamped_gap(60, link),
                clamped_gap(p::max_fps_for(link), link),
                "{link:?} an over-fast request should land on the ceiling"
            );
            assert_eq!(
                clamped_gap(1, link),
                Duration::from_millis(1000),
                "{link:?}"
            );
            // Zero is not a rate. It must not divide by zero or ask for a
            // busy-loop; it lands on the slowest setting instead.
            assert_eq!(
                clamped_gap(0, link),
                Duration::from_millis(1000),
                "{link:?}"
            );
        }
    }

    #[test]
    fn the_radio_is_paced_more_gently_than_the_cable() {
        assert!(clamped_gap(60, Link::Dongle) > clamped_gap(60, Link::Wired));
    }
}
