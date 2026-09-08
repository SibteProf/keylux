//! AULA F75 driver.

pub mod keymap;
pub mod protocol;

use std::thread;
use std::time::{Duration, Instant};

use crate::color::ChannelOrder;
use crate::device::{Frame, KeyPos, RgbDevice};
use crate::transport::Transport;
use crate::{Error, Result};

use protocol as p;

pub struct F75 {
    io: Transport,
    layout: Vec<KeyPos>,
    order: ChannelOrder,
    /// Enforces the minimum gap between colour writes.
    last_write: Option<Instant>,
}

impl F75 {
    pub fn open() -> Result<Self> {
        Self::open_with(ChannelOrder::Rgb)
    }

    pub fn open_with(order: ChannelOrder) -> Result<Self> {
        let io = Transport::open(p::VENDOR_ID, p::PRODUCT_ID, p::REPORT_ID, p::PACKET_LEN)?;
        Ok(Self {
            io,
            layout: keymap::layout(),
            order,
            last_write: None,
        })
    }

    pub fn hid_path(&self) -> &str {
        &self.io.path
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
            thread::sleep(Duration::from_millis(20));
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
        thread::sleep(Duration::from_millis(p::CONFIG_SETTLE_MS));
        self.last_write = Some(Instant::now());
        Ok(())
    }

    /// Is the board currently in per-key mode?
    pub fn is_per_key_mode(&self) -> Result<bool> {
        let cfg = self.read_config()?;
        Ok(cfg[p::mode::OFF_A] == p::mode::PER_KEY[0]
            && cfg[p::mode::OFF_B] == p::mode::PER_KEY[1]
            && cfg[p::mode::OFF_C] == p::mode::PER_KEY[2])
    }

    /// Sleep out the remainder of the minimum inter-write gap.
    ///
    /// Writing faster than the firmware can absorb starves its key-scanning
    /// loop and the keyboard stops responding to keypresses until replugged.
    fn respect_gap(&mut self) {
        if let Some(prev) = self.last_write {
            let min = Duration::from_millis(p::MIN_FRAME_GAP_MS);
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
        "AULA F75"
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
        p::MAX_FPS
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
        self.respect_gap();
        let payload = self.encode_stream(frame);
        self.io.send(&p::build_packet(
            p::cmd::STREAM,
            p::ADDR,
            p::STREAM_LEN as u16,
            Some(&payload),
        ))
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
}
