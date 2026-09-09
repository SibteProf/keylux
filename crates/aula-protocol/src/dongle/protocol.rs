//! AULA 2.4 GHz receiver wire protocol.
//!
//! A DIFFERENT protocol from the wired one, on a different USB device. Decoded
//! from a USBPcap capture of AULA F75 v2.0; see `docs/PROTOCOL.md`.
//!
//! # SAFETY: this driver sends ONE command
//!
//! Only `0x88` (lighting), which is volatile — it paints and stores nothing.
//!
//! `0x04` carries a 128-byte config block and is DECODED BUT NEVER SENT. An
//! earlier version wrote the wired protocol's config block through it to force
//! per-key mode, on the inference that both links share a byte-identical
//! layout. It stopped a keyboard typing and needed a firmware reflash. That
//! block is non-volatile storage on a device that also holds the key matrix,
//! and it was written blind because this link has no known readback.
//!
//! The rule the wired path already had, and this one now shares: never write a
//! config block you have not read back and validated first.
//!
//! Frame (HID **output** report, 20 bytes):
//!
//! ```text
//! byte 0      report id, always 0x13
//! byte 1      command family (0x88 = lighting)
//! byte 2      total chunks in this message
//! byte 3      chunk index, 0-based
//! byte 4      16 + payload length in this chunk
//! bytes 5..18 payload, zero padded
//! byte 19     checksum: sum of bytes 0..18, mod 256
//! ```

use crate::color::Rgb;

pub const REPORT_ID: u8 = 0x13;
pub const FRAME_LEN: usize = 20;
/// Payload bytes carried by one frame.
pub const CHUNK_PAYLOAD: usize = 14;
/// EXPERIMENT: the largest LIGHTING message seen was 6 chunks, but the config
/// family reaches 28, so the chunking itself is not limited to 6. Testing
/// whether 0x88 accepts a wider message — if it does, a whole multi-colour
/// board fits and full-board effects become possible.
pub const MAX_CHUNKS: usize = 28;
pub const MAX_PAYLOAD: usize = CHUNK_PAYLOAD * MAX_CHUNKS; // 84

/// `byte 4` is the payload length plus this. Verified against all 448 lighting
/// frames in the capture, with zero padding violations.
const LEN_BIAS: u8 = 16;

pub mod cmd {
    /// Per-key colour painting. Its length byte carries a `0x10` flag.
    ///
    /// Volatile: it paints, it stores nothing. This is the only command this
    /// driver sends.
    pub const LIGHTING: u8 = 0x88;

    /// Writes the 128-byte config block. **NOT SENT — see the module docs.**
    ///
    /// Kept as documentation of what was decoded, not as something to call.
    /// Writing a config block assembled from the wired protocol's bytes stopped
    /// a keyboard typing and needed a firmware reflash.
    pub const WRITE_CONFIG: u8 = 0x04;
}

#[cfg(test)]
/// Frame builder for the config family, whose length byte is RAW.
///
/// **Deliberately private.** The framing is decoded and tested, but nothing in
/// this crate may send a config write until there is a readback to do
/// read-modify-write against — see [`crate::dongle::Dongle::ensure_per_key_mode`].
/// Making it private is the guard: a future caller has to come here and read
/// this comment before it can reach for it.
///
/// Only the lighting family biases the length byte. Captured config chunks
/// carry `0x0e` for a full 14-byte payload and `0x02` for the two-byte tail
/// holding the `5A A5` signature.
fn build_raw_frame(cmd: u8, chunks: u8, index: u8, payload: &[u8]) -> [u8; FRAME_LEN] {
    debug_assert!(payload.len() <= CHUNK_PAYLOAD, "payload overruns the frame");
    let mut f = [0u8; FRAME_LEN];
    f[0] = REPORT_ID;
    f[1] = cmd;
    f[2] = chunks;
    f[3] = index;
    f[4] = payload.len() as u8;
    f[5..5 + payload.len()].copy_from_slice(payload);
    f[FRAME_LEN - 1] = checksum(&f);
    f
}

#[cfg(test)]
/// Split a 128-byte config block into the ten frames that would carry it.
///
/// **Deliberately private, and not called outside tests.** See above.
fn chunk_config(config: &[u8]) -> Vec<[u8; FRAME_LEN]> {
    let parts: Vec<&[u8]> = config.chunks(CHUNK_PAYLOAD).collect();
    let total = parts.len() as u8;
    parts
        .iter()
        .enumerate()
        .map(|(i, p)| build_raw_frame(cmd::WRITE_CONFIG, total, i as u8, p))
        .collect()
}

/// The length byte of the idle tick, which carries no payload.
///
/// The vendor driver sends `13 88 01 00 23 00..00` on a tick where nothing
/// changed. `0x23` does not fit the length rule — it is an opcode, not a
/// length — so it is spelled out rather than derived.
const IDLE_LEN_BYTE: u8 = 0x23;

/// Sustainable tick period.
///
/// MEASURED from the vendor driver while it was **animating**: message starts
/// land 46-47 ms apart, the same rate the wired path uses. That is no
/// coincidence — it is the same firmware and the same key-scanning 8051 at the
/// far end, so the floor is the same.
///
/// This was 108 ms once, taken from the wrong part of the capture: an idle or
/// music-reactive stretch, which is event-driven and paces itself. Reading a
/// rate off a window where nothing was animating made the driver twice as slow
/// as the hardware, and made each delta twice as large — the frames were
/// further apart, so more keys had changed between them. Animation looked
/// terrible for both reasons at once.
pub const TICK_MS: u64 = 46;

/// Spacing between the chunks of one message.
///
/// The vendor's measured minimum: 13.0 ms, mean 14.0 across 90 gaps. It was 6
/// for a while, chased for frame rate, before it became clear that the link's
/// limit is total throughput rather than intra-message spacing — see
/// `MAX_CHUNKS_PER_SEC`, which is what actually governs the rate now.
///
/// Staying at the vendor's number costs nothing here and keeps this driver
/// inside behaviour the hardware is known to tolerate. The failure mode for
/// going faster is the key-scan starvation the wired path documents: the board
/// lights perfectly while dropping and doubling keypresses.
pub const PACKET_GAP_MS: u64 = 13;

/// Resend everything if nothing has been sent for this long.
///
/// Two jobs. The firmware repaints itself behind the driver — the wired path
/// documents the same hazard — and a delta-only driver is defenceless against
/// it: once it believes the board is correct it goes silent, so the cache still
/// says "already showing that" and nothing ever corrects it.
///
/// The interval is short because a battery keyboard appears to DIM its LEDs
/// when the host stops talking. At two seconds a static colour visibly pulsed:
/// dim, then bright again as the resend landed, once per interval. Keeping the
/// gap under the dimming timeout holds the brightness steady.
///
/// This only fires when nothing changed, so it costs nothing during an
/// animation and never interrupts one.
pub const KEEPALIVE_MS: u64 = 700;

/// The link's actual capacity, in 20-byte chunks per second.
///
/// MEASURED by bisection on hardware: a full-board frame is 8 chunks, and
/// streaming it at 8 FPS (64 chunks/s) is clean while 12 FPS (96) and 16 FPS
/// (128) visibly fight — the board flickers between the frame being sent and
/// whatever it had, because frames arrive faster than they can be applied.
///
/// For scale, the vendor driver's own streaming runs at about **14 chunks/s**:
/// 1-2 chunks every ~107 ms. It never streams a whole board, and this is why.
/// Its smooth wireless effects are firmware effects with the radio idle.
///
/// CHUNKS per second, not frames, because that is what the link actually
/// carries. A sparse effect whose frame is two chunks may run at 30 FPS on the
/// same budget that limits an eight-chunk full-board frame to 8. Capping frames
/// instead would punish the cheap effect for the expensive one's cost.
pub const MAX_CHUNKS_PER_SEC: u64 = 64;

/// Frames one STREAMED tick may send.
///
/// A whole message. This is a COHERENCE decision and it beats raw frame rate.
///
/// A frame whose delta does not fit is truncated, so the board ends up showing
/// several animation timestamps at once — different keys stranded at different
/// moments. That reads as skipping and "moving too fast", and no amount of
/// frame rate fixes it. Measured on the smoke test's wave: a 42-byte budget
/// completed 0 of 200 frames, sending ~14 of the ~61 keys that changed. An
/// 84-byte budget with a 4-level palette completes 98%.
///
/// The cost is real: six chunks spend 60 ms of gaps, so the tick stretches and
/// the rate settles around 14-16 FPS rather than 21. The vendor driver makes
/// exactly this trade, letting its own tick run out to ~104 ms for a 4-chunk
/// message. Complete frames at 15 FPS look like motion; torn frames at 21 look
/// broken.
pub const STREAM_FRAMES_PER_TICK: usize = MAX_CHUNKS;

/// Frames a one-off full repaint may send.
pub const MAX_FRAMES_PER_TICK: usize = MAX_CHUNKS;

// Compile-time, because the cost of getting these wrong is not a rendering
// artefact.
const _: () = {
    assert!(
        STREAM_FRAMES_PER_TICK <= MAX_CHUNKS,
        "a message cannot carry more chunks than the protocol allows"
    );
    assert!(
        MAX_FRAMES_PER_TICK >= STREAM_FRAMES_PER_TICK,
        "a one-off repaint may not be stingier than a streamed frame"
    );
    assert!(
        STREAM_FRAMES_PER_TICK >= 1,
        "a frame needs at least one chunk"
    );
};

/// One run of LEDs sharing a colour.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub color: Rgb,
    pub leds: Vec<u8>,
}

impl Group {
    /// Bytes this group occupies in a payload: colour, count, then the indices.
    pub fn encoded_len(&self) -> usize {
        4 + self.leds.len()
    }
}

/// Build one 20-byte frame. `payload` must fit [`CHUNK_PAYLOAD`].
pub fn build_frame(cmd: u8, chunks: u8, index: u8, payload: &[u8]) -> [u8; FRAME_LEN] {
    debug_assert!(payload.len() <= CHUNK_PAYLOAD, "payload overruns the frame");
    let mut f = [0u8; FRAME_LEN];
    f[0] = REPORT_ID;
    f[1] = cmd;
    f[2] = chunks;
    f[3] = index;
    f[4] = LEN_BIAS + payload.len() as u8;
    let n = payload.len().min(CHUNK_PAYLOAD);
    f[5..5 + n].copy_from_slice(&payload[..n]);
    f[FRAME_LEN - 1] = checksum(&f);
    f
}

/// The frame the vendor sends on a tick with no colour change.
///
/// NOT SENT by this driver, and kept only because its bytes are decoded and
/// checked against the capture. Sending it every idle tick made a solid colour
/// flash: light, dark, light, dark. `0x23` was ASSUMED to mean "nothing
/// changed" — it appears 176 times in a music-reactive capture, where keys go
/// dark constantly, so it may equally mean "blank". Until something verifies
/// which, the driver stays quiet when it has nothing to say.
pub fn idle_frame() -> [u8; FRAME_LEN] {
    let mut f = [0u8; FRAME_LEN];
    f[0] = REPORT_ID;
    f[1] = cmd::LIGHTING;
    f[2] = 1;
    f[3] = 0;
    f[4] = IDLE_LEN_BYTE;
    f[FRAME_LEN - 1] = checksum(&f);
    f
}

/// Sum of every byte but the last, mod 256.
pub fn checksum(frame: &[u8]) -> u8 {
    frame[..FRAME_LEN - 1]
        .iter()
        .fold(0u8, |a, b| a.wrapping_add(*b))
}

/// Smallest useful group: a colour, a count, and one LED.
const MIN_GROUP: usize = 5;

/// Encode colour groups into a flat payload, splitting to fit the budget.
///
/// Each group is `[R, G, B, count, index * count]`. A group too big for the
/// remaining room is **truncated**, not skipped: it is a colour plus a list, so
/// any prefix of it is a valid, complete instruction.
///
/// Refusing to split was a real bug. A solid board is one group of eighty, far
/// past a streamed frame's budget, so nothing was encodable and nothing got
/// sent — the common case produced an empty payload.
///
/// Returns the payload and the groups actually encoded, so the caller knows
/// precisely which LEDs went out.
pub fn encode_groups(groups: &[Group], budget: usize) -> (Vec<u8>, Vec<Group>) {
    let mut out = Vec::with_capacity(budget.min(MAX_PAYLOAD));
    let mut sent = Vec::new();
    for g in groups {
        if g.leds.is_empty() {
            continue;
        }
        let room = budget.saturating_sub(out.len());
        if room < MIN_GROUP {
            break;
        }
        // The count field is one byte, so a group can never exceed 255 anyway.
        let fits = (room - 4).min(g.leds.len()).min(255);
        let leds = &g.leds[..fits];
        out.push(g.color.r);
        out.push(g.color.g);
        out.push(g.color.b);
        out.push(fits as u8);
        out.extend_from_slice(leds);
        sent.push(Group {
            color: g.color,
            leds: leds.to_vec(),
        });
        if fits < g.leds.len() {
            break; // budget ran out mid-group
        }
    }
    (out, sent)
}

/// Split a payload into the frames that carry it.
pub fn chunk(cmd: u8, payload: &[u8]) -> Vec<[u8; FRAME_LEN]> {
    if payload.is_empty() {
        return Vec::new();
    }
    let parts: Vec<&[u8]> = payload.chunks(CHUNK_PAYLOAD).collect();
    let total = parts.len() as u8;
    parts
        .iter()
        .enumerate()
        .map(|(i, p)| build_frame(cmd, total, i as u8, p))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every one of these came off the wire. If a change breaks them it has
    /// broken the protocol, not the test.
    #[test]
    fn frames_match_the_captured_bytes() {
        // 13 88 01 00 15 00 ff ff 01 37 00.. e7  — LED 0x37 cyan
        let g = Group {
            color: Rgb::new(0, 255, 255),
            leds: vec![0x37],
        };
        let (payload, sent) = encode_groups(&[g], CHUNK_PAYLOAD);
        assert_eq!(sent.len(), 1);
        let frames = chunk(cmd::LIGHTING, &payload);
        assert_eq!(frames.len(), 1);
        assert_eq!(
            frames[0].to_vec(),
            vec![
                0x13, 0x88, 0x01, 0x00, 0x15, 0x00, 0xff, 0xff, 0x01, 0x37, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0xe7
            ]
        );
    }

    #[test]
    fn two_leds_of_one_colour_match_the_capture() {
        // 13 88 01 00 16 00 00 ff 02 24 2a 00.. 01
        let g = Group {
            color: Rgb::new(0, 0, 255),
            leds: vec![0x24, 0x2a],
        };
        let (payload, _) = encode_groups(&[g], CHUNK_PAYLOAD);
        let f = chunk(cmd::LIGHTING, &payload)[0];
        assert_eq!(f[4], 0x16);
        assert_eq!(&f[5..11], &[0x00, 0x00, 0xff, 0x02, 0x24, 0x2a]);
        assert_eq!(f[19], 0x01);
    }

    #[test]
    fn two_groups_pack_into_one_frame() {
        // 13 88 01 00 1a 00 ff ff 01 43 ff ff 00 01 27 00.. 1e
        let groups = [
            Group {
                color: Rgb::new(0, 255, 255),
                leds: vec![0x43],
            },
            Group {
                color: Rgb::new(255, 255, 0),
                leds: vec![0x27],
            },
        ];
        let (payload, sent) = encode_groups(&groups, CHUNK_PAYLOAD);
        assert_eq!(sent.len(), 2);
        let f = chunk(cmd::LIGHTING, &payload)[0];
        assert_eq!(f[4], 0x1a);
        assert_eq!(f[19], 0x1e);
    }

    #[test]
    fn the_idle_tick_matches_the_capture() {
        let f = idle_frame();
        assert_eq!(&f[..5], &[0x13, 0x88, 0x01, 0x00, 0x23]);
        assert!(f[5..19].iter().all(|b| *b == 0));
        assert_eq!(f[19], 0xbf, "the capture's idle checksum");
    }

    /// A whole-board apply: one colour, 80 keys, six chunks.
    ///
    /// This is the cheapest possible full frame, and the shape the vendor uses
    /// for the same job. It is the floor every richer frame is measured against.
    #[test]
    fn a_solid_full_board_costs_six_chunks() {
        let g = Group {
            color: Rgb::new(255, 255, 0),
            leds: (0..80).collect(),
        };
        assert_eq!(g.encoded_len(), 84, "3 colour + 1 count + 80 indices");
        let (payload, sent) = encode_groups(&[g], MAX_PAYLOAD);
        assert_eq!(payload.len(), 84);
        assert_eq!(sent.len(), 1, "it must fit whole");
        let frames = chunk(cmd::LIGHTING, &payload);
        assert_eq!(frames.len(), 6);
        for (i, f) in frames.iter().enumerate() {
            assert_eq!(f[2], 6, "chunk count");
            assert_eq!(f[3], i as u8, "chunk index");
            assert_eq!(f[19], checksum(f));
        }
    }

    /// A frame is all-or-nothing, so the budget has to hold a realistic one.
    ///
    /// `0x88` turns off every key it does not mention, so a truncated message
    /// does not mean "the rest arrives next tick" — it means the rest goes
    /// DARK. The budget must therefore fit a whole board with a realistic
    /// number of colours, not merely bound the bytes.
    #[test]
    fn the_budget_fits_a_whole_multicolour_board() {
        // 80 lit keys spread over 20 colours: 20*4 + 80 = 160 bytes.
        let groups: Vec<Group> = (0..20u8)
            .map(|i| Group {
                color: Rgb::new(i.wrapping_mul(11), i, 255 - i),
                leds: (0..4).map(|j| i * 4 + j).collect(),
            })
            .collect();
        let need: usize = groups.iter().map(|g| g.encoded_len()).sum();
        assert_eq!(need, 160);
        assert!(
            need <= MAX_PAYLOAD,
            "a 20-colour board needs {need} bytes but the cap is {MAX_PAYLOAD}; \
             every key past the cut would go dark"
        );

        let (payload, sent) = encode_groups(&groups, MAX_PAYLOAD);
        assert_eq!(sent.len(), groups.len(), "the whole frame must fit");
        assert_eq!(payload.len(), need);
        let encoded: usize = sent.iter().map(|g| g.encoded_len()).sum();
        assert_eq!(encoded, payload.len());
    }

    /// Truncation still has to be safe when a frame genuinely cannot fit.
    #[test]
    fn an_oversized_frame_is_truncated_not_corrupted() {
        let groups: Vec<Group> = (0..60u8)
            .map(|i| Group {
                color: Rgb::new(i, i, i),
                leds: vec![i, i + 1, i + 2, i + 3, i + 4],
            })
            .collect();
        let (payload, sent) = encode_groups(&groups, MAX_PAYLOAD);
        assert!(payload.len() <= MAX_PAYLOAD);
        assert!(sent.len() < groups.len(), "it should have run out of room");
        let encoded: usize = sent.iter().map(|g| g.encoded_len()).sum();
        assert_eq!(
            encoded,
            payload.len(),
            "what came back must describe the bytes"
        );
    }

    #[test]
    fn checksums_hold_for_every_frame_shape() {
        for n in 0..=CHUNK_PAYLOAD {
            let payload: Vec<u8> = (0..n as u8).collect();
            let f = build_frame(cmd::LIGHTING, 1, 0, &payload);
            assert_eq!(f[19], checksum(&f));
            assert_eq!(f[4], 16 + n as u8);
        }
    }

    /// Config framing, checked against the captured bytes.
    ///
    /// The capture's block is the vendor's rainbow config, so the mode bytes
    /// differ from ours — but the framing, chunk count and signature tail must
    /// match exactly.
    #[test]
    fn config_chunks_match_the_captured_framing() {
        let frames = chunk_config(&crate::f75::protocol::PER_KEY_CONFIG);
        assert_eq!(frames.len(), 10, "128 bytes is ten 14-byte chunks");

        // 13 04 0a 00 0e ...
        assert_eq!(&frames[0][..5], &[0x13, 0x04, 0x0a, 0x00, 0x0e]);
        for (i, f) in frames.iter().enumerate() {
            assert_eq!(f[2], 10, "chunk count");
            assert_eq!(f[3], i as u8, "chunk index");
            assert_eq!(f[19], checksum(f), "checksum");
        }

        // The tail carries just the signature: 13 04 0a 09 02 5a a5
        let last = frames[9];
        assert_eq!(&last[..7], &[0x13, 0x04, 0x0a, 0x09, 0x02, 0x5a, 0xa5]);
    }

    /// The length byte is raw here and biased in the lighting family. Mixing
    /// them up produces frames the firmware silently ignores.
    #[test]
    fn config_length_is_raw_where_lighting_is_biased() {
        let raw = build_raw_frame(cmd::WRITE_CONFIG, 1, 0, &[1, 2, 3]);
        assert_eq!(raw[4], 3);
        let lit = build_frame(cmd::LIGHTING, 1, 0, &[1, 2, 3]);
        assert_eq!(lit[4], 3 + 16);
    }
}
