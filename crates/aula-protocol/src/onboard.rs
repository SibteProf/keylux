//! Native F75 receiver lighting. Effect algorithms remain on the keyboard.
//!
//! Read commands and field offsets come from AULA HUB's bytechMechanical
//! driver, checked against device readbacks in reference/. Configuration writes
//! always modify a fresh receiver read; wired fixture blocks are never used.

use crate::{Error, Result, Rgb};

pub const EFFECTS: &[(u16, &str)] = &[
    (0, "Off"),
    (1, "Static"),
    (2, "Breathing"),
    (3, "Dream Rainbow"),
    (4, "Instant Trigger"),
    (5, "Rain Walk"),
    (7, "Ripple Spread"),
    (8, "Starry Sky"),
    (10, "Flowing Stream"),
    (11, "Drifting"),
    (12, "Shadow Follow"),
    (13, "Sine Wave"),
    (15, "Rotating Windmill"),
    (16, "Colorful Waterfall"),
    (17, "Flower Bloom"),
    (277, "Custom"),
];

pub fn effect_name(mode: u16) -> &'static str {
    EFFECTS
        .iter()
        .find(|(id, _)| *id == mode)
        .map_or("Unknown", |(_, name)| name)
}

fn invalid(message: &str) -> Error {
    Error::Onboard(message.into())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LightingSettings {
    pub mode: u16,
    /// Native 0..20; displayed as 1..21 by the vendor UI.
    pub brightness: u8,
    /// Four-bit native value, 0..15.
    pub speed: u8,
    pub multicolor: bool,
}

#[derive(Clone, Debug)]
pub struct OnboardLighting {
    pub active: LightingSettings,
    /// Each mode's saved parameters; choosing a mode need not replace them.
    pub presets: Vec<LightingSettings>,
    pub palettes: Vec<(u16, [Rgb; 7])>,
}

impl OnboardLighting {
    pub fn from_readbacks(config: &ConfigBlock, colors: &[u8]) -> Result<Self> {
        if colors.len() != 490 {
            return Err(invalid("incomplete color table (expected 490 bytes)"));
        }
        let mut presets = Vec::new();
        let mut palettes = Vec::new();
        for &(mode, _) in EFFECTS {
            presets.push(config.settings_for(mode)?);
            if mode > 0 && mode < 23 {
                let offset = mode as usize * 21;
                let mut palette = [Rgb::BLACK; 7];
                for (i, color) in palette.iter_mut().enumerate() {
                    let p = offset + i * 3;
                    *color = Rgb::new(colors[p], colors[p + 1], colors[p + 2]);
                }
                palettes.push((mode, palette));
            }
        }
        Ok(Self {
            active: config.active()?,
            presets,
            palettes,
        })
    }
}

/// A complete, signature-checked receiver read. There is no constructor for
/// caller-provided default values and no API for arbitrary byte edits.
pub struct ConfigBlock([u8; 128]);

impl ConfigBlock {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let data: [u8; 128] = bytes
            .try_into()
            .map_err(|_| invalid("incomplete config (expected 128 bytes)"))?;
        if data[126..] != [0x5a, 0xa5] {
            return Err(invalid("config signature is not 5A A5; refusing to use it"));
        }
        // The observed receiver layout's lighting table begins at byte 58.
        // Refuse an unfamiliar layout instead of searching for a coincidental marker.
        if data[56..58] != [0xff, 0xff] {
            return Err(invalid("unrecognized receiver lighting-table layout"));
        }
        Ok(Self(data))
    }

    pub fn bytes(&self) -> &[u8; 128] {
        &self.0
    }

    pub fn active(&self) -> Result<LightingSettings> {
        self.settings_for(u16::from_be_bytes([self.0[9], self.0[10]]))
    }

    pub fn settings_for(&self, mode: u16) -> Result<LightingSettings> {
        if !EFFECTS.iter().any(|(id, _)| *id == mode) {
            return Err(invalid("unrecognized firmware mode; no settings changed"));
        }
        let (brightness, speed, multicolor) = if mode == 0 || mode == 277 {
            (0, 0, true)
        } else {
            let offset = 56 + mode as usize * 2;
            let options = self.0[offset + 1];
            if self.0[offset] > 20 || ![0, 7].contains(&(options & 15)) {
                return Err(invalid("unrecognized firmware brightness/color parameters"));
            }
            (self.0[offset], options >> 4, options & 15 == 7)
        };
        Ok(LightingSettings {
            mode,
            brightness,
            speed,
            multicolor,
        })
    }

    pub fn with_settings(&self, settings: LightingSettings) -> Result<Self> {
        self.settings_for(settings.mode)?;
        if settings.brightness > 20 || settings.speed > 15 {
            return Err(invalid("brightness must be 0..20 and speed 0..15"));
        }
        let mut data = self.0;
        [data[9], data[10]] = settings.mode.to_be_bytes();
        if settings.mode != 0 && settings.mode != 277 {
            let offset = 56 + settings.mode as usize * 2;
            data[offset] = settings.brightness;
            data[offset + 1] = settings.speed << 4 | if settings.multicolor { 7 } else { 0 };
        }
        Self::parse(&data)
    }
}

/// Checks every packet before exposing any bytes to the configuration parser.
pub(crate) struct Reply {
    command: u8,
    size: usize,
    parts: Vec<Option<Vec<u8>>>,
}

impl Reply {
    pub fn new(command: u8, size: usize) -> Self {
        Self {
            command,
            size,
            parts: vec![None; size.div_ceil(14)],
        }
    }

    pub fn push(&mut self, frame: &[u8]) -> Result<Option<Vec<u8>>> {
        if frame.first() != Some(&0x13) || frame.get(1) != Some(&self.command) {
            return Ok(None);
        }
        if frame.len() != 20 || frame[..19].iter().fold(0u8, |a, b| a.wrapping_add(*b)) != frame[19]
        {
            return Err(invalid("bad receiver reply checksum or report size"));
        }
        let (count, index, length) = (frame[2] as usize, frame[3] as usize, frame[4] as usize);
        if count != self.parts.len() || index >= count || length != (self.size - index * 14).min(14)
        {
            return Err(invalid(
                "unexpected receiver reply count, index, or payload size",
            ));
        }
        let part = frame[5..5 + length].to_vec();
        if self.parts[index].as_ref().is_some_and(|old| old != &part) {
            return Err(invalid("conflicting duplicate receiver packet"));
        }
        self.parts[index] = Some(part);
        if self.parts.iter().any(Option::is_none) {
            return Ok(None);
        }
        Ok(Some(
            self.parts
                .iter()
                .flat_map(|p| p.as_ref().unwrap().iter().copied())
                .collect(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "requires the physical receiver; briefly changes its effect and restores the exact initial config"]
    fn receiver_native_effect_round_trip_preserves_full_configuration() {
        let mut kb = crate::dongle::open(&[]).unwrap();
        let before = kb.read_onboard_config().unwrap();
        let original = before.active().unwrap();
        let alternate = if original.mode == 3 { 11 } else { 3 };
        let chosen = before.settings_for(alternate).unwrap();
        let apply_result = kb.set_onboard_lighting(chosen);
        // Restoration is an explicit second transaction, using its own fresh
        // validated read even if the first apply's verification failed.
        let restore_result = kb.set_onboard_lighting(original);
        restore_result.expect("Could not restore the starting native effect");
        assert_eq!(kb.read_onboard_config().unwrap().bytes(), before.bytes());
        apply_result.expect("Native effect apply was not verified");
    }

    fn config() -> ConfigBlock {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../reference/f75-receiver-config-readback.json"
        ))
        .unwrap();
        let bytes: Vec<u8> = fixture["payload"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap() as u8)
            .collect();
        ConfigBlock::parse(&bytes).unwrap()
    }

    #[test]
    fn captured_receiver_configuration_decodes_current_effect() {
        let c = config();
        assert_eq!(
            c.active().unwrap(),
            LightingSettings {
                mode: 11,
                brightness: 1,
                speed: 3,
                multicolor: true
            }
        );
        for &(mode, _) in EFFECTS {
            c.settings_for(mode).unwrap();
        }
    }

    #[test]
    fn native_selection_preserves_keymaps_and_every_other_effect() {
        let c = config();
        for &(mode, _) in EFFECTS {
            let mut settings = c.settings_for(mode).unwrap();
            settings.brightness = 10;
            settings.speed = 4;
            settings.multicolor = false;
            let changed = c.with_settings(settings).unwrap();
            for i in 0..128 {
                let own_parameters = mode > 0
                    && mode < 23
                    && [56 + mode as usize * 2, 57 + mode as usize * 2].contains(&i);
                if ![9, 10].contains(&i) && !own_parameters {
                    assert_eq!(changed.bytes()[i], c.bytes()[i], "mode {mode}, offset {i}");
                }
            }
            assert_eq!(changed.active().unwrap().mode, mode);
        }
    }

    #[test]
    fn already_selected_settings_are_byte_identical() {
        let c = config();
        assert_eq!(
            c.with_settings(c.active().unwrap()).unwrap().bytes(),
            c.bytes()
        );
    }

    #[test]
    fn truncated_corrupted_and_unfamiliar_configs_are_rejected() {
        let c = config();
        assert!(ConfigBlock::parse(&c.bytes()[..114]).is_err());
        let mut b = *c.bytes();
        b[126] = 0;
        assert!(ConfigBlock::parse(&b).is_err());
        let mut b = *c.bytes();
        b[56] = 0;
        assert!(ConfigBlock::parse(&b).is_err());
        let mut settings = c.active().unwrap();
        settings.mode = 6;
        assert!(c.with_settings(settings).is_err());
        settings.mode = 11;
        settings.speed = 16;
        assert!(c.with_settings(settings).is_err());
        settings.speed = 0;
        settings.brightness = 21;
        assert!(c.with_settings(settings).is_err());
    }

    #[test]
    fn captured_color_packets_assemble_and_decode_the_actual_palettes() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../reference/f75-receiver-color-readback.json"
        ))
        .unwrap();
        let reports = fixture["response"]["reports"].as_array().unwrap();
        let mut reply = Reply::new(0x49, 490);
        let mut assembled = None;
        for r in reports.iter().rev() {
            let mut frame = vec![0x13];
            frame.extend(
                r["bytes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_u64().unwrap() as u8),
            );
            assembled = reply.push(&frame).unwrap();
        }
        let state = OnboardLighting::from_readbacks(&config(), &assembled.unwrap()).unwrap();
        assert_eq!(state.palettes.len(), 14);
        let expected = [
            Rgb::new(255, 0, 0),
            Rgb::new(0, 255, 0),
            Rgb::new(0, 0, 255),
            Rgb::new(255, 255, 0),
            Rgb::new(255, 0, 255),
            Rgb::new(0, 255, 255),
            Rgb::new(255, 255, 255),
        ];
        assert!(state.palettes.iter().all(|(_, p)| *p == expected));
    }

    #[test]
    fn a_missing_config_chunk_never_produces_a_writable_block() {
        let c = config();
        let mut reply = Reply::new(0x44, 128);
        for (index, part) in c.bytes().chunks(14).enumerate() {
            if index == 1 {
                continue;
            }
            let frame = crate::dongle::protocol::build_raw_frame(0x44, 10, index as u8, part);
            assert!(reply.push(&frame).unwrap().is_none());
        }
    }

    #[test]
    fn corrupt_and_conflicting_reply_packets_are_rejected() {
        let c = config();
        let frame = crate::dongle::protocol::build_raw_frame(0x44, 10, 0, &c.bytes()[..14]);
        let mut reply = Reply::new(0x44, 128);
        assert!(reply.push(&frame).unwrap().is_none());
        assert!(reply.push(&frame).unwrap().is_none());
        let mut corrupt = frame;
        corrupt[5] ^= 1;
        assert!(reply.push(&corrupt).is_err());
        let mut conflicting = frame;
        conflicting[5] ^= 1;
        conflicting[19] = crate::dongle::protocol::checksum(&conflicting);
        assert!(reply.push(&conflicting).is_err());
    }
}
