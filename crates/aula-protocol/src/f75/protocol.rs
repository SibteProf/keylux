//! AULA F75 wire protocol.
//!
//! Every value here was verified against the hardware, most of it decoded from
//! USBPcap captures of the vendor driver. See `docs/PROTOCOL.md`.
//!
//! Packet layout (HID feature report, 520 bytes):
//!
//! ```text
//! byte 0      report id, always 0x06
//! byte 1      command
//! bytes 2..5  address (always 00 00 01 00 for colour and config)
//! bytes 6..7  payload length, little-endian
//! bytes 8..   payload, zero padded to 512
//! ```

/// Wired USB-C mode. The 2.4 GHz dongle and Bluetooth do not expose this.
pub const VENDOR_ID: u16 = 0x258a; // Sinowealth
pub const PRODUCT_ID: u16 = 0x010c; // AULA F75

pub const REPORT_ID: u8 = 0x06;
pub const HEADER_LEN: usize = 8;
pub const PAYLOAD_LEN: usize = 512;
pub const PACKET_LEN: usize = HEADER_LEN + PAYLOAD_LEN; // 520

/// Commands.
///
/// There is no single "set the colours" command — there are three, with
/// DIFFERENT payload layouts. This is the part no public source documents.
pub mod cmd {
    /// Write the 128-byte config block.
    pub const WRITE_CONFIG: u8 = 0x04;
    /// Read the 128-byte config block.
    pub const READ_CONFIG: u8 = 0x84;

    /// Static per-key colours. 384 bytes, PLANAR, persists across reboots.
    ///
    /// Renders one frame correctly, but repeated writes blank the whole board
    /// including the charging indicator. Never use this for animation.
    pub const WRITE_STATIC: u8 = 0x06;
    /// Read back the static colour table.
    pub const READ_STATIC: u8 = 0x86;

    /// Live streaming. 378 bytes, INTERLEAVED RGB. This is the animation path.
    ///
    /// Found by capturing the vendor driver's music-reactive modes; it is not
    /// inferable from a static capture because the driver only uses it while
    /// animating.
    pub const STREAM: u8 = 0x08;

    /// Solid single colour. 512 bytes; the firmware takes one colour and paints
    /// every key with it. A per-key frame sent here collapses to a flat colour.
    pub const WRITE_SOLID: u8 = 0x0a;

    /// Key matrix table: 4 bytes per entry, HID usage code in byte 3.
    /// This is where the LED ordering came from.
    pub const READ_MATRIX: u8 = 0x83;
}

/// Colour and config share this address. The COMMAND selects the region.
///
/// The address is not a general pointer: `WRITE_SOLID` at `00 00 00 00` targets
/// a staging buffer that accepts writes and reads them back byte-perfect while
/// driving nothing at all. That buffer is the trap the published notes lead you
/// into — a flawless round-trip there proves nothing.
pub const ADDR: [u8; 4] = [0x00, 0x00, 0x01, 0x00];

pub const CONFIG_LEN: usize = 0x0080; // 128

/// Static path: 128 slots x 3 bytes, planar (all R, then all G, then all B).
pub const STATIC_LEN: usize = 0x0180; // 384
pub const STATIC_SLOTS: usize = 128;

/// Streaming path: 126 slots x 3 bytes, interleaved.
pub const STREAM_LEN: usize = 0x017a; // 378
pub const STREAM_SLOTS: usize = STREAM_LEN / 3; // 126

/// Sustainable frame rate.
///
/// The vendor driver streams its own effects at 21.5 FPS with 46.5 ms between
/// writes and never dips below 44.7 ms. Pushing harder starves the 8051's
/// key-scanning loop: the keyboard stops responding to keypresses until it is
/// replugged. 60 FPS is not achievable on this hardware.
///
/// The gap is the source of truth and the frame rate is derived from it. They
/// were once independent constants and drifted apart: the gap said 40 ms, which
/// is 25 FPS — faster than the vendor driver ever writes. The board still lit
/// correctly, but the scan loop was starved just enough to drop the occasional
/// keypress and to report others twice.
pub const MIN_FRAME_GAP_MS: u64 = 46;
pub const MAX_FPS: u32 = (1000 / MIN_FRAME_GAP_MS) as u32;

/// Resend an unchanged frame at least this often.
///
/// Identical frames are otherwise skipped, which costs nothing visually and
/// takes a still image — or a paused effect — down to no bus traffic at all.
/// The periodic resend is insurance: if the firmware ever repaints itself, the
/// board recovers on its own instead of staying wrong until something moves.
pub const KEEPALIVE_MS: u64 = 2_000;

// Compile-time, not tests: getting these wrong does not produce a rendering
// artefact, it produces a keyboard that drops and repeats keypresses. That
// should never reach a build at all.
const _: () = {
    assert!(
        MAX_FPS as u64 * MIN_FRAME_GAP_MS <= 1000,
        "the advertised frame rate needs a tighter gap than the driver enforces"
    );
    // The vendor driver never writes closer together than 44.7 ms, measured
    // across three captures. Below that is uncharted, and the LEDs cannot show
    // the difference anyway.
    assert!(
        MIN_FRAME_GAP_MS >= 45,
        "writing faster than the vendor driver starves the key-scan loop"
    );
    // Skipping unchanged frames is only safe if something eventually resends,
    // and the resend has to be rare enough to cost nothing.
    assert!(
        KEEPALIVE_MS > MIN_FRAME_GAP_MS * 10,
        "the keepalive is frequent enough to be a second frame rate"
    );
};

/// A config write triggers an ASYNCHRONOUS repaint inside the firmware that
/// lands hundreds of milliseconds later and overwrites whatever frame was sent
/// in the meantime. Wait this long after one before streaming.
pub const CONFIG_SETTLE_MS: u64 = 400;

/// A valid config block ends with this signature.
///
/// The firmware returns truncated, mostly-zero buffers while it is busy —
/// reliably reproducible by polling as the mode knob is pressed. Since setting
/// the mode is read-modify-write, accepting a glitched read would write zeros
/// over the entire config block.
pub const CONFIG_MAGIC: [u8; 2] = [0x5a, 0xa5];

/// Config bytes that select the lighting mode.
///
/// All THREE bytes matter. With only 9 and 10 set the board goes completely
/// dark, indicator included — byte 59 is the first effect entry's speed/colour
/// nibble and differs between the two modes.
pub mod mode {
    pub const OFF_A: usize = 9;
    pub const OFF_B: usize = 10;
    pub const OFF_C: usize = 59;

    pub const PER_KEY: [u8; 3] = [0x01, 0x15, 0x47];
    pub const SOLID: [u8; 3] = [0x00, 0x01, 0x40];
}

/// The full config block captured from the vendor driver at the moment its
/// per-key apply worked.
///
/// Written verbatim rather than patching individual bytes: the three mode bytes
/// are known, but writing a known-good whole block avoids depending on the rest
/// of the block being in a sane state.
#[rustfmt::skip]
pub const PER_KEY_CONFIG: [u8; CONFIG_LEN] = [
    0x00, 0x03, 0x03, 0x01, 0x00, 0x00, 0x04, 0x04, 0x07, 0x01, 0x15, 0x20,
    0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x02, 0x01, 0x00, 0xff,
    0x0a, 0x00, 0x01, 0x00, 0x01, 0x00, 0x03, 0x01, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0x09, 0x47,
    0x09, 0x47, 0x09, 0x47, 0x09, 0x47, 0x09, 0x47, 0x09, 0x47, 0x09, 0x47,
    0x09, 0x47, 0x09, 0x47, 0x09, 0x47, 0x09, 0x47, 0x09, 0x47, 0x09, 0x47,
    0x09, 0x47, 0x09, 0x47, 0x09, 0x37, 0x09, 0x37, 0x09, 0x37, 0x09, 0x37,
    0x07, 0x47, 0x07, 0x47, 0x07, 0x44, 0x07, 0x44, 0x07, 0x44, 0x07, 0x44,
    0x07, 0x44, 0x07, 0x44, 0x07, 0x44, 0x04, 0x09, 0x04, 0x04, 0x04, 0x04,
    0x04, 0x04, 0x04, 0x04, 0x04, 0x04, 0x5a, 0xa5,
];

/// Build a 520-byte feature-report packet.
pub fn build_packet(cmd: u8, addr: [u8; 4], len: u16, payload: Option<&[u8]>) -> Vec<u8> {
    let mut pkt = vec![0u8; PACKET_LEN];
    pkt[0] = REPORT_ID;
    pkt[1] = cmd;
    pkt[2..6].copy_from_slice(&addr);
    pkt[6] = (len & 0xff) as u8;
    pkt[7] = (len >> 8) as u8;
    if let Some(p) = payload {
        let n = p.len().min(PAYLOAD_LEN);
        pkt[HEADER_LEN..HEADER_LEN + n].copy_from_slice(&p[..n]);
    }
    pkt
}

/// Does this block look like a real config read?
pub fn is_valid_config(cfg: &[u8]) -> bool {
    cfg.len() == CONFIG_LEN
        && cfg[CONFIG_LEN - 2] == CONFIG_MAGIC[0]
        && cfg[CONFIG_LEN - 1] == CONFIG_MAGIC[1]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_header_is_well_formed() {
        let pkt = build_packet(cmd::STREAM, ADDR, STREAM_LEN as u16, None);
        assert_eq!(pkt.len(), PACKET_LEN);
        assert_eq!(pkt[0], REPORT_ID);
        assert_eq!(pkt[1], 0x08);
        assert_eq!(&pkt[2..6], &[0x00, 0x00, 0x01, 0x00]);
        // 0x017a little-endian
        assert_eq!(pkt[6], 0x7a);
        assert_eq!(pkt[7], 0x01);
    }

    #[test]
    fn payload_is_copied_and_clamped() {
        let big = vec![0xabu8; PAYLOAD_LEN + 64];
        let pkt = build_packet(cmd::WRITE_STATIC, ADDR, STATIC_LEN as u16, Some(&big));
        assert_eq!(pkt.len(), PACKET_LEN);
        assert_eq!(pkt[HEADER_LEN], 0xab);
        assert_eq!(pkt[PACKET_LEN - 1], 0xab);
    }

    #[test]
    fn embedded_config_is_per_key_and_valid() {
        assert!(is_valid_config(&PER_KEY_CONFIG));
        assert_eq!(PER_KEY_CONFIG[mode::OFF_A], mode::PER_KEY[0]);
        assert_eq!(PER_KEY_CONFIG[mode::OFF_B], mode::PER_KEY[1]);
        assert_eq!(PER_KEY_CONFIG[mode::OFF_C], mode::PER_KEY[2]);
    }

    #[test]
    fn zeroed_config_is_rejected() {
        assert!(!is_valid_config(&[0u8; CONFIG_LEN]));
    }

    #[test]
    fn stream_slot_count_is_exact() {
        assert_eq!(STREAM_LEN % 3, 0);
        assert_eq!(STREAM_SLOTS, 126);
    }
}
