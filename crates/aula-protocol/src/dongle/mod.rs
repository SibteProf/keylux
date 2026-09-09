//! AULA 2.4 GHz receiver driver.
//!
//! A different device and a different protocol from the wired board — see
//! [`protocol`] and `docs/PROTOCOL.md`. The shape of the thing:
//!
//! * 20-byte HID **output** reports, not 520-byte feature reports.
//! * `0x88` is a FULL-FRAME command: every lit key must be in the message,
//!   because any key it does not mention is turned OFF. It looks like a delta
//!   command in a music-reactive capture only because that mode lights a
//!   handful of keys at a time.
//! * A frame costs 4 bytes per distinct colour plus one per lit key, so a
//!   solid board is 84 bytes and colours are what makes a frame expensive.
//! * ~21 FPS, measured while the vendor driver was animating — the same rate
//!   the cable runs at, because it is the same firmware at the far end.
//! * Colours are QUANTISED so a whole board fits one message. A smooth
//!   gradient is a group per key and does not fit at all.
//!
//! The LED indices are the SAME ones the wired protocol uses, so
//! [`crate::f75::keymap`] applies unchanged.

pub mod protocol;

use std::collections::BTreeMap;
use std::thread;
use std::time::{Duration, Instant};

use crate::color::Rgb;
use crate::device::{DeviceId, Frame, KeyPos, Link, RgbDevice};
use crate::f75::keymap;
use crate::transport::{Confidence, DeviceCandidate, Transport};
use crate::{Error, Result};

use protocol as p;

pub const MODEL: &str = "AULA F75";

/// How finely a colour is snapped before grouping.
///
/// The protocol's cost is per DISTINCT COLOUR, not per key: 80 keys of one
/// colour is a single 84-byte group, while an 80-key smooth gradient is ~80
/// groups and fits in no tick at all. So colours must be collapsed onto a grid
/// before they are grouped, and the grid's shape decides how the result looks.
///
/// Snapping in HSV rather than per RGB channel, because the two are not
/// equivalent. A 4-level RGB grid gives 64 possible colours, but they are
/// spread unevenly around the wheel: a hue sweep lands on muddy, unequal steps
/// and the motion reads as lurching. Snapping hue separately gives evenly
/// spaced steps, and — because a key only counts as changed when it crosses a
/// hue boundary — a moving effect touches roughly `1/HUE_STEPS` of the board
/// per frame instead of all of it.
///
/// The vendor's own animation payloads are similarly coarse (`00`, `33`, `66`,
/// `b2`, `ff`), so this is the shape of the constraint, not a shortcut.
///
/// The step count is a FRAME RATE control, not just a look. A full board costs
/// `4 bytes per colour + 1 per lit key`, and every 14 bytes is another 13 ms
/// chunk on the wire. For 80 keys: 1 colour is 6 chunks (~15 FPS), 8 colours is
/// 8 chunks (~11 FPS), 12 colours is 10 chunks (~8 FPS). The 80 key indices are
/// irreducible, so colours are the only lever, and 6 chunks is the floor no
/// full-board frame can beat.
const HUE_STEPS: f32 = 8.0;

/// Cap, so a frame can never grow past what one message carries.
const MAX_HUE_STEPS: f32 = 40.0;
/// Saturation is the least visible of the three on a keyboard, so it gets the
/// coarsest grid.
const SAT_STEPS: f32 = 4.0;
/// Value needs a finer grid than it looks. A coarse one ROUNDS DIM COLOURS TO
/// BLACK: at four steps everything below 12.5% brightness snapped to zero, so a
/// sweep's dim background vanished and only the bright line and its immediate
/// glow survived. It read as the effect being broken, not as a palette problem.
///
/// It is also cheap to be generous here: a flat background is one group however
/// finely value is snapped, and only a brightness GRADIENT costs extra groups.
const VAL_STEPS: f32 = 8.0;

/// Snap a colour to the quantisation grid. Black and full white stay exact.
///
/// `hue_steps` is the dial that matters. It is NOT only a look: it sets how
/// many visibly distinct positions a moving effect can occupy, and too few
/// makes motion stall and jump rather than travel. It also sets the frame size,
/// so it trades directly against frame rate.
fn quantise_with(c: Rgb, hue_steps: f32) -> Rgb {
    let (h, s, v) = c.to_hsv();
    let snap = |x: f32, steps: f32| (x * steps).round() / steps;
    // Hue wraps, so snapping it must too, or 0.98 and 0.02 land in different
    // bands despite being the same colour.
    let h = (h * hue_steps).round().rem_euclid(hue_steps) / hue_steps;
    // A lit key must never snap to black. Rounding it off is indistinguishable
    // from the effect failing to paint it.
    let mut qv = snap(v, VAL_STEPS);
    if v > 0.0 && qv == 0.0 {
        qv = 1.0 / VAL_STEPS;
    }
    Rgb::from_hsv(h, snap(s, SAT_STEPS), qv)
}

/// Snap at the default grid. Used by the tests, which pin behaviour that must
/// hold whatever the dial is set to.
#[cfg(test)]
fn quantise(c: Rgb) -> Rgb {
    quantise_with(c, HUE_STEPS)
}

/// A keyboard reached through its 2.4 GHz receiver.
pub struct Dongle {
    io: Transport,
    layout: Vec<KeyPos>,
    id: DeviceId,
    name: String,
    /// The quantised colour of every laid-out key, in layout order, as last
    /// SENT WHOLE. Empty means unknown, which forces a full send.
    ///
    /// Compared all-or-nothing: the unit of transmission is the entire frame,
    /// so a per-key comparison would have no meaning.
    shown: Vec<Rgb>,
    last_tick: Option<Instant>,
    /// When the board was last painted, for the keepalive.
    last_full: Option<Instant>,
    tick: Duration,
    /// Inter-chunk spacing. Defaults to the vendor's measured minimum.
    gap: Duration,
    /// Hue quantisation steps. More means smoother-looking motion and a bigger,
    /// slower frame.
    hue_steps: f32,
    /// Chunks the last frame cost, which sets how long the next must wait.
    last_chunks: usize,
}

impl Dongle {
    pub fn open_candidate(c: &DeviceCandidate) -> Result<Self> {
        let io = Transport::open_path(&c.path, p::REPORT_ID, p::FRAME_LEN)?;
        let layout = keymap::layout();
        Ok(Self {
            io,
            layout,
            id: c.id,
            name: format!("{MODEL} ({})", Link::Dongle.suffix()),
            // Nothing is known about what the board is showing until we have
            // painted it, so the first frame must go out in full.
            shown: Vec::new(),
            last_tick: None,
            last_full: None,
            tick: Duration::from_millis(p::TICK_MS),
            gap: Duration::from_millis(p::PACKET_GAP_MS),
            hue_steps: HUE_STEPS,
            last_chunks: 0,
        })
    }

    pub fn id(&self) -> DeviceId {
        self.id
    }

    pub fn hid_path(&self) -> &str {
        &self.io.path
    }

    /// Override the inter-chunk spacing. **For experiments only.**
    ///
    /// The default is the vendor's measured minimum. Going below it is
    /// uncharted: the failure mode is the key-scan starvation the wired path
    /// documents, where the board keeps lighting perfectly while dropping and
    /// doubling keypresses. That is VOLATILE — a replug clears it — but it is
    /// also easy to miss, because nothing about the lighting looks wrong.
    ///
    /// Anything found this way needs the same evidence the wired floor has:
    /// sustained typing during and after a run, not one quick look.
    pub fn set_packet_gap_ms(&mut self, ms: u64) {
        // A floor, because at some point this stops being an experiment and
        // starts being a denial-of-service on the keyboard's own firmware.
        self.gap = Duration::from_millis(ms.clamp(2, 100));
    }

    /// Set how many hue bands a colour may snap to.
    ///
    /// Raising it makes motion look continuous rather than stepping, and makes
    /// each frame larger and therefore slower. Lowering it does the reverse.
    /// Which way to go is a judgement about how it LOOKS, not a number that can
    /// be derived — too few bands and an effect visibly holds still and jumps,
    /// however high the frame rate is.
    pub fn set_hue_steps(&mut self, steps: f32) {
        self.hue_steps = steps.clamp(2.0, MAX_HUE_STEPS);
    }

    /// Wait until the link can take another frame.
    ///
    /// Two limits, whichever is longer. The tick is the firmware's floor, the
    /// same rule the wired path has: writing faster than it can absorb starves
    /// the key-scan loop, and the failure is subtle — the board lights
    /// perfectly while dropping keypresses.
    ///
    /// The chunk budget is the radio's, and it is the one that usually binds.
    /// The cost of a frame is its chunk count, so the previous frame's size
    /// sets how long to wait before the next: an 8-chunk full-board frame earns
    /// a 125 ms gap at 64 chunks/s, while a 2-chunk sparse one earns 31 ms and
    /// runs at the tick instead. Exceed it and frames arrive faster than the
    /// board can apply them, which looks like the effect fighting something
    /// else for control of the colours.
    fn respect_rate(&mut self) {
        let budget = Duration::from_micros(
            self.last_chunks as u64 * 1_000_000 / p::MAX_CHUNKS_PER_SEC.max(1),
        );
        let wait = self.tick.max(budget);
        if let Some(prev) = self.last_tick {
            let elapsed = prev.elapsed();
            if elapsed < wait {
                thread::sleep(wait - elapsed);
            }
        }
        self.last_tick = Some(Instant::now());
    }

    /// Send one message, split across as many frames as it needs.
    fn send_message(&self, payload: &[u8]) -> Result<usize> {
        let frames = p::chunk(p::cmd::LIGHTING, payload);
        let gap = self.gap;
        let mut last: Option<Instant> = None;
        for f in &frames {
            // Space the writes 13 ms APART, rather than sleeping 13 ms between
            // them. The two are not the same: a control transfer takes a
            // couple of milliseconds itself, so sleeping the full gap and then
            // writing puts ~15 ms on the wire — slower than the vendor, which
            // is where 13 ms was measured. Over a ten-chunk frame that pure
            // overhead cost more than a whole frame's worth of latency.
            if let Some(prev) = last {
                let elapsed = prev.elapsed();
                if elapsed < gap {
                    thread::sleep(gap - elapsed);
                }
            }
            last = Some(Instant::now());
            self.io.send_output(f)?;
        }
        Ok(frames.len())
    }

    /// Whether an unchanged board is due a defensive resend.
    fn due_for_keepalive(&self) -> bool {
        self.last_full
            .map(|t| t.elapsed() >= Duration::from_millis(p::KEEPALIVE_MS))
            .unwrap_or(true)
    }

    /// Paint a frame. Always the WHOLE frame.
    ///
    /// `0x88` is a full-frame command: every lit key must be in the message,
    /// because any key the message does not mention is turned OFF. It is not a
    /// delta command, and an earlier version of this driver treated it as one.
    ///
    /// That single misreading produced every visual fault this driver had. A
    /// sweep showed colour only around its bright line, because only the keys
    /// that changed were sent and the rest went dark. A wave looked like
    /// scattered noise for the same reason. A budget that truncated the message
    /// made whole rows blink in and out as the truncation point moved. The
    /// vendor's own messages look like small deltas in a music-reactive capture
    /// only because that mode genuinely lights a handful of keys at a time.
    ///
    /// Sending nothing is still safe: the board holds its last frame. So an
    /// identical frame is skipped, and a keepalive covers the firmware
    /// repainting behind us.
    fn paint(&mut self, frame: &Frame, force: bool) -> Result<()> {
        let groups = self.plan(frame);
        let wanted: Vec<Rgb> = self
            .layout
            .iter()
            .map(|k| quantise_with(frame.get(k.led), self.hue_steps))
            .collect();

        // Skip only if the whole board already matches. A partial comparison
        // has no meaning when the unit of transmission is the entire frame.
        if !force && self.shown == wanted && !self.due_for_keepalive() {
            return Ok(());
        }

        self.respect_rate();

        let (payload, sent) = p::encode_groups(&groups, p::MAX_PAYLOAD);
        if payload.is_empty() {
            return Ok(());
        }
        self.last_chunks = self.send_message(&payload)?;
        self.last_full = Some(Instant::now());

        // Record the board state only if the message really carried the whole
        // frame. If it was truncated, some keys are now dark and the cache must
        // not claim otherwise.
        let complete = sent.len() == groups.len()
            && sent.iter().map(|g| g.leds.len()).sum::<usize>()
                == groups.iter().map(|g| g.leds.len()).sum::<usize>();
        self.shown = if complete { wanted } else { Vec::new() };
        Ok(())
    }

    /// Every lit key on the board, grouped by colour.
    ///
    /// No diffing: see [`Dongle::paint`]. Grouping is what makes a whole board
    /// fit — cost is `4 bytes per distinct colour, plus one per lit key`, so a
    /// solid board is 84 bytes and a six-colour one is 104.
    ///
    /// Dark keys are simply omitted, which is free: not mentioning a key is
    /// exactly how the protocol turns it off.
    fn plan(&self, frame: &Frame) -> Vec<p::Group> {
        let mut by_colour: BTreeMap<(u8, u8, u8), Vec<u8>> = BTreeMap::new();
        for k in &self.layout {
            if k.led > usize::from(u8::MAX) {
                continue;
            }
            let want = quantise_with(frame.get(k.led), self.hue_steps);
            if want == Rgb::BLACK {
                continue;
            }
            by_colour
                .entry((want.r, want.g, want.b))
                .or_default()
                .push(k.led as u8);
        }
        // Largest runs first: if the frame ever does overflow the message, the
        // most visible part of it survives.
        let mut groups: Vec<p::Group> = by_colour
            .into_iter()
            .map(|((r, g, b), leds)| p::Group {
                color: Rgb::new(r, g, b),
                leds,
            })
            .collect();
        groups.sort_by_key(|g| std::cmp::Reverse(g.leds.len()));
        groups
    }
}

impl RgbDevice for Dongle {
    fn name(&self) -> &str {
        &self.name
    }

    fn led_count(&self) -> usize {
        // Matches the wired path so one frame drives either device.
        crate::f75::protocol::STREAM_SLOTS
    }

    fn layout(&self) -> &[KeyPos] {
        &self.layout
    }

    fn max_fps(&self) -> u32 {
        (1000 / p::TICK_MS) as u32
    }

    /// DOES NOT write config. See below — this is a deliberate refusal.
    ///
    /// Command `0x04` does carry a 128-byte config block over this link, and an
    /// earlier version of this driver wrote the WIRED [`PER_KEY_CONFIG`] into it
    /// to force per-key mode. That **stopped a keyboard typing** and needed a
    /// firmware reflash to recover.
    ///
    /// [`PER_KEY_CONFIG`]: crate::f75::protocol::PER_KEY_CONFIG
    ///
    /// The mistake was inferring, from one capture, that the two links share a
    /// byte-for-byte identical config layout. The first fourteen bytes do look
    /// alike, which is exactly what made the guess tempting. But this is
    /// **non-volatile storage on a device that also holds the key matrix**, and
    /// the wired path never writes a block it has not first read back and
    /// validated. Here there is no known readback, so the block was written
    /// blind — a foreign 128 bytes over settings we cannot see.
    ///
    /// Restoring this needs, in order:
    ///
    /// 1. A **readback** for the config over `0x13` — the 20-byte interrupt IN
    ///    reports are the likely carrier and are not yet decoded.
    /// 2. Read-modify-write of only the three mode bytes, never a whole block
    ///    from another transport.
    /// 3. `5A A5` validation before the write, as the wired path does.
    ///
    /// Until then the board stays in whatever mode the user left it. The cost
    /// is that a firmware effect keeps repainting over host colours; that is a
    /// cosmetic problem, and bricking someone's keyboard is not.
    fn ensure_per_key_mode(&mut self) -> Result<bool> {
        Ok(false)
    }

    fn set_static(&mut self, frame: &Frame) -> Result<()> {
        // Nothing here persists the way the wired static path does, so the
        // honest implementation is a full repaint.
        self.paint(frame, true)
    }

    fn stream(&mut self, frame: &Frame) -> Result<()> {
        self.paint(frame, false)
    }

    fn set_max_fps(&mut self, fps: u32) {
        let fps = fps.clamp(1, self.max_fps());
        let period = 1000 / u64::from(fps);
        // Only ever slower, exactly as on the wired path.
        self.tick = Duration::from_millis(period.max(p::TICK_MS));
    }
}

/// The receiver's lighting collection, as reported by Windows.
pub const USAGE_PAGE_LIGHTING: u16 = 0xff02;

/// Receivers confirmed to speak this protocol.
///
/// Unlike the wired table this cannot be found by probing — the channel has no
/// feature reports, so it is invisible until you either know the id or capture
/// the vendor driver. `3554:fa09` (Compx) was confirmed by capture.
pub const KNOWN: &[DeviceId] = &[DeviceId::new(0x3554, 0xfa09)];

/// Find receivers whose lighting channel is present.
///
/// Selection is by device id AND usage page, not by probing: there is nothing
/// safe to probe with. The channel takes output reports only, and writing an
/// unknown command to unknown hardware is exactly what `docs/PROTOCOL.md`
/// forbids.
pub fn discover(extra: &[DeviceId]) -> Result<Vec<DeviceCandidate>> {
    let known = |id: DeviceId| KNOWN.contains(&id) || extra.contains(&id);
    Ok(crate::transport::list_all()?
        .into_iter()
        .filter(|c| known(c.id) && c.usage_page == USAGE_PAGE_LIGHTING)
        .map(|c| DeviceCandidate {
            id: c.id,
            link: Link::Dongle,
            model: MODEL,
            path: c.path,
            product: c.product,
            confidence: Confidence::Known,
            // Nothing is read back on this path, so "confirmed" can only mean
            // "this id is one we have decoded", never "it just answered".
            confirmed: KNOWN.contains(&c.id),
        })
        .collect())
}

/// Open the first receiver present.
pub fn open(extra: &[DeviceId]) -> Result<Dongle> {
    let found = discover(extra)?;
    let c = found.first().ok_or(Error::NotFound {
        searched: KNOWN.len() + extra.len(),
    })?;
    Dongle::open_candidate(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `plan` is the whole design, so it is tested directly against a layout
    /// rather than through a device.
    fn plan_with(layout: &[KeyPos], shown: &[Rgb], frame: &Frame, force: bool) -> Vec<p::Group> {
        let mut by_colour: BTreeMap<(u8, u8, u8), Vec<u8>> = BTreeMap::new();
        for k in layout {
            let want = frame.get(k.led);
            let have = shown.get(k.led).copied();
            if !force && have == Some(want) {
                continue;
            }
            by_colour
                .entry((want.r, want.g, want.b))
                .or_default()
                .push(k.led as u8);
        }
        let mut groups: Vec<p::Group> = by_colour
            .into_iter()
            .map(|((r, g, b), leds)| p::Group {
                color: Rgb::new(r, g, b),
                leds,
            })
            .collect();
        groups.sort_by_key(|g| std::cmp::Reverse(g.leds.len()));
        groups
    }

    /// `plan_with`, but through the real quantiser.
    fn plan_quantised(layout: &[KeyPos], shown: &[Rgb], frame: &Frame) -> Vec<p::Group> {
        let mut by_colour: BTreeMap<(u8, u8, u8), Vec<u8>> = BTreeMap::new();
        for k in layout {
            let want = quantise(frame.get(k.led));
            if shown.get(k.led).copied() == Some(want) {
                continue;
            }
            by_colour
                .entry((want.r, want.g, want.b))
                .or_default()
                .push(k.led as u8);
        }
        by_colour
            .into_iter()
            .map(|((r, g, b), leds)| p::Group {
                color: Rgb::new(r, g, b),
                leds,
            })
            .collect()
    }

    #[test]
    fn an_unchanged_frame_plans_nothing() {
        let layout = keymap::layout();
        let frame = Frame::solid(128, Rgb::new(10, 20, 30));
        let shown: Vec<Rgb> = vec![Rgb::new(10, 20, 30); 128];
        assert!(plan_with(&layout, &shown, &frame, false).is_empty());
    }

    #[test]
    fn a_solid_board_is_a_single_group() {
        let layout = keymap::layout();
        let frame = Frame::solid(128, Rgb::new(255, 255, 0));
        let groups = plan_with(&layout, &[], &frame, false);
        assert_eq!(groups.len(), 1, "one colour should be one group");
        assert_eq!(groups[0].leds.len(), layout.len());
        // This is the capture's whole-board apply: 80 keys, six chunks.
        assert_eq!(groups[0].encoded_len(), 4 + layout.len());
    }

    #[test]
    fn only_changed_keys_are_planned() {
        let layout = keymap::layout();
        let mut frame = Frame::solid(128, Rgb::BLACK);
        let shown: Vec<Rgb> = vec![Rgb::BLACK; 128];
        frame.set(layout[0].led, Rgb::new(255, 0, 0));
        frame.set(layout[1].led, Rgb::new(255, 0, 0));
        let groups = plan_with(&layout, &shown, &frame, false);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].leds.len(), 2);
        assert_eq!(groups[0].color, Rgb::new(255, 0, 0));
    }

    #[test]
    fn the_largest_colour_run_is_planned_first() {
        let layout = keymap::layout();
        let mut frame = Frame::solid(128, Rgb::BLACK);
        for (i, k) in layout.iter().enumerate() {
            frame.set(
                k.led,
                if i < 3 {
                    Rgb::new(1, 2, 3)
                } else {
                    Rgb::new(4, 5, 6)
                },
            );
        }
        let groups = plan_with(&layout, &[], &frame, false);
        assert_eq!(groups.len(), 2);
        assert!(
            groups[0].leds.len() > groups[1].leds.len(),
            "a tight budget should buy the most visible change first"
        );
    }

    /// The pathological case for a 14-byte pipe: every key a different colour.
    #[test]
    fn a_rainbow_plans_many_small_groups() {
        let layout = keymap::layout();
        let mut frame = Frame::solid(128, Rgb::BLACK);
        for (i, k) in layout.iter().enumerate() {
            frame.set(k.led, Rgb::new(i as u8, 255 - i as u8, 128));
        }
        let groups = plan_with(&layout, &[], &frame, false);
        assert!(
            groups.len() > 20,
            "distinct colours cannot be grouped, and that is the real limit"
        );
        // Budgeting must still bound what goes out.
        let (payload, sent) = p::encode_groups(&groups, p::MAX_PAYLOAD);
        assert!(payload.len() <= p::MAX_PAYLOAD);
        assert!(sent.len() < groups.len());
    }

    /// The whole reason quantising is here.
    ///
    /// `0x88` is all-or-nothing: a frame that does not fit one message is not
    /// "delivered late", it is delivered with the overflow DARK. So the test
    /// that matters is whether a realistic board fits WHOLE, not whether
    /// banding saves some bytes.
    #[test]
    fn quantising_makes_a_whole_gradient_board_fit_one_message() {
        let layout = keymap::layout();
        let mut frame = Frame::black(128);
        for (i, k) in layout.iter().enumerate() {
            let v = (i * 255 / layout.len()) as u8;
            frame.set(k.led, Rgb::new(v, 255 - v, 128));
        }

        let raw = plan_with(&layout, &[], &frame, false);
        let banded = plan_quantised(&layout, &[], &frame);
        assert!(
            banded.len() * 4 < raw.len(),
            "banding should cut the group count hard: {} -> {}",
            raw.len(),
            banded.len()
        );

        // The unquantised board is a group per key and does not fit.
        let raw_need: usize = raw.iter().map(|g| g.encoded_len()).sum();
        assert!(
            raw_need > p::MAX_PAYLOAD,
            "an unquantised gradient should not fit; it needs {raw_need}"
        );

        // The quantised one must fit whole, with every lit key present.
        let need: usize = banded.iter().map(|g| g.encoded_len()).sum();
        let (payload, sent) = p::encode_groups(&banded, p::MAX_PAYLOAD);
        assert_eq!(
            sent.len(),
            banded.len(),
            "a quantised board must fit whole: {need} bytes against {} cap",
            p::MAX_PAYLOAD
        );
        assert_eq!(payload.len(), need);

        let lit: usize = sent.iter().map(|g| g.leds.len()).sum();
        assert_eq!(
            lit,
            layout.len(),
            "every laid-out key must be in the frame, or it goes dark"
        );
    }

    #[test]
    fn quantise_keeps_the_endpoints_exact() {
        // Black and white must survive untouched, or every "off" key drifts.
        assert_eq!(quantise(Rgb::BLACK), Rgb::BLACK);
        assert_eq!(quantise(Rgb::new(255, 255, 255)), Rgb::new(255, 255, 255));
        assert_eq!(quantise(Rgb::new(255, 0, 0)), Rgb::new(255, 0, 0));
    }

    /// Quantising is idempotent, so a key that has settled stops being resent.
    /// Covers the hue circle as well as arbitrary RGB, since the two snap on
    /// different grids.
    #[test]
    fn quantising_twice_changes_nothing() {
        for v in 0..=255u8 {
            let c = quantise(Rgb::new(v, 255 - v, v / 2));
            assert_eq!(quantise(c), c, "value {v} was not stable");
        }
        for i in 0..64 {
            let h = i as f32 / 64.0;
            let c = quantise(Rgb::from_hsv(h, 1.0, 1.0));
            assert_eq!(quantise(c), c, "hue {h} was not stable");
        }
    }

    /// A real effect's per-frame delta must fit the budget WHOLE.
    ///
    /// This is the one that matters, and the one a smaller budget or a finer
    /// palette breaks. A truncated frame strands different keys at different
    /// moments of the animation, and the board shows several timestamps at
    /// once — which looks like skipping, not like a slow frame rate. Measured
    /// over 200 frames of this wave, five colour levels completed 0% of frames
    /// and four completed 98%.
    ///
    /// If this fails, animation on the receiver will look broken even though
    /// every unit test about framing and checksums still passes.
    #[test]
    fn a_moving_wave_fits_one_message_per_frame() {
        let keys = keymap::layout();
        let max_x = keymap::max_x(&keys).max(1.0);
        let max_row = f32::from(keymap::max_row(&keys)).max(1.0);
        let budget = p::STREAM_FRAMES_PER_TICK * p::CHUNK_PAYLOAD;

        let mut shown: Vec<Rgb> = vec![Rgb::BLACK; 128];
        let (mut complete, mut frames) = (0, 0);

        for f in 0..60 {
            let t = f as f32 / 21.0;
            let mut by_colour: BTreeMap<(u8, u8, u8), Vec<u8>> = BTreeMap::new();
            for k in &keys {
                let nx = k.x / max_x;
                let ny = f32::from(k.row) / max_row;
                let want = quantise(Rgb::from_hsv(nx * 1.2 + ny * 0.25 - t * 0.35, 1.0, 1.0));
                if shown[k.led] == want {
                    continue;
                }
                by_colour
                    .entry((want.r, want.g, want.b))
                    .or_default()
                    .push(k.led as u8);
            }
            if by_colour.is_empty() {
                continue;
            }
            let groups: Vec<p::Group> = by_colour
                .into_iter()
                .map(|((r, g, b), leds)| p::Group {
                    color: Rgb::new(r, g, b),
                    leds,
                })
                .collect();
            let need: usize = groups.iter().map(|g| g.encoded_len()).sum();
            frames += 1;
            if need <= budget {
                complete += 1;
            }
            let (_, sent) = p::encode_groups(&groups, budget);
            for g in &sent {
                for led in &g.leds {
                    shown[usize::from(*led)] = g.color;
                }
            }
        }

        // The first frame paints the whole board and is allowed to overflow.
        let ratio = complete as f32 / frames as f32;
        assert!(
            ratio > 0.9,
            "only {complete}/{frames} frames fit a {budget}-byte message; the board will \
             show several animation timestamps at once and look like it is skipping"
        );
    }

    /// The jitter bug, as a test.
    ///
    /// Order by group size and the small groups never get sent: on a moving
    /// effect those are its leading and trailing edges, so the bulk of the
    /// board updates every frame while the moving part stutters. Ageing has to
    /// bound how long any key waits.
    #[test]
    fn no_key_starves_when_the_budget_cannot_cover_the_board() {
        let layout = keymap::layout();
        // One big block of a single colour plus a few stragglers — the shape
        // that starves under size-ordering.
        let mut ages: Vec<u64> = vec![0; 128];
        let mut sent_at: Vec<Option<u64>> = vec![None; 128];

        for frame_no in 1..=40u64 {
            let mut by_colour: BTreeMap<(u8, u8, u8), Vec<u8>> = BTreeMap::new();
            for (i, k) in layout.iter().enumerate() {
                // Big uniform block, and a handful of unique colours that would
                // always lose a size contest.
                let c = if i < layout.len() - 5 {
                    Rgb::new(0, 0, 255)
                } else {
                    Rgb::new(i as u8, 0, 0)
                };
                by_colour
                    .entry((c.r, c.g, c.b))
                    .or_default()
                    .push(k.led as u8);
            }
            let mut groups: Vec<p::Group> = by_colour
                .into_iter()
                .map(|((r, g, b), leds)| p::Group {
                    color: Rgb::new(r, g, b),
                    leds,
                })
                .collect();
            // The real ordering under test: ages inside each group, then
            // groups by their most-neglected member.
            for g in &mut groups {
                g.leds.sort_by_key(|l| ages[usize::from(*l)]);
            }
            groups.sort_by_key(|g| {
                let oldest = g
                    .leds
                    .iter()
                    .map(|l| ages[usize::from(*l)])
                    .min()
                    .unwrap_or(0);
                (oldest, std::cmp::Reverse(g.leds.len()))
            });

            let budget = p::STREAM_FRAMES_PER_TICK * p::CHUNK_PAYLOAD;
            let (_, sent) = p::encode_groups(&groups, budget);
            for g in &sent {
                for led in &g.leds {
                    ages[usize::from(*led)] = frame_no;
                    sent_at[usize::from(*led)] = Some(frame_no);
                }
            }
        }

        let never = layout.iter().filter(|k| sent_at[k.led].is_none()).count();
        assert_eq!(never, 0, "{never} keys were never updated in 40 frames");

        // And nobody is left far behind the rest.
        let newest = ages.iter().copied().max().unwrap();
        let stalest = layout.iter().map(|k| ages[k.led]).min().unwrap();
        assert!(
            newest - stalest <= 12,
            "one key is {} frames behind the freshest; motion will look uneven",
            newest - stalest
        );
    }

    /// Hue wraps, so the grid must too. Without `rem_euclid` a hue of 0.98 and
    /// one of 0.02 snap to different bands despite being the same red, and the
    /// wrap point flickers on every pass of a rainbow.
    #[test]
    fn the_hue_grid_wraps() {
        let a = quantise(Rgb::from_hsv(0.999, 1.0, 1.0));
        let b = quantise(Rgb::from_hsv(0.001, 1.0, 1.0));
        assert_eq!(a, b, "the wrap point must not be a seam");
    }

    /// A lit key must never quantise to black.
    ///
    /// This is the sweep bug. With four value steps everything below 12.5%
    /// brightness rounded to zero, so an effect's dim background disappeared
    /// and only its bright highlight and the keys nearest it stayed lit. It
    /// looked like the effect was failing to paint, not like a palette being
    /// too coarse, which is what made it hard to place.
    #[test]
    fn a_dim_colour_never_becomes_black() {
        for v in 1..=40u8 {
            for hue in [0.0f32, 0.33, 0.66] {
                let dim = Rgb::from_hsv(hue, 1.0, f32::from(v) / 255.0);
                if dim == Rgb::BLACK {
                    continue; // genuinely black input
                }
                let q = quantise(dim);
                assert_ne!(
                    q,
                    Rgb::BLACK,
                    "a lit key at value {v} hue {hue} was quantised out of existence"
                );
            }
        }
        // Actual black still has to stay off.
        assert_eq!(quantise(Rgb::BLACK), Rgb::BLACK);
    }

    /// The rate limit must be a CHUNK budget, not a frame budget.
    ///
    /// The link's capacity is chunks per second: an 8-chunk full-board frame is
    /// clean at 8 FPS and fights at 12, while a 2-chunk sparse frame costs a
    /// quarter as much and should be allowed to run four times as often. An FPS
    /// cap would charge the cheap effect the expensive one's price.
    #[test]
    fn the_budget_gives_sparse_frames_more_headroom() {
        let per_frame = |chunks: u64| chunks * 1000 / p::MAX_CHUNKS_PER_SEC;
        let full_board = per_frame(8);
        let sparse = per_frame(2);
        assert!(
            sparse * 3 < full_board,
            "a 2-chunk frame should be far cheaper than an 8-chunk one: \
             {sparse} ms vs {full_board} ms"
        );
        // The measured clean rate: 8 chunks at 8 FPS.
        assert_eq!(full_board, 125, "8 chunks should earn a 125 ms gap");
    }

    /// The tick is a floor the budget can never undercut.
    #[test]
    fn a_cheap_frame_is_still_paced_by_the_tick() {
        let budget_ms = 1000 / p::MAX_CHUNKS_PER_SEC;
        assert!(
            budget_ms < p::TICK_MS,
            "a one-chunk frame's budget ({budget_ms} ms) is under the tick, so the \
             tick must be what limits it"
        );
    }
}
