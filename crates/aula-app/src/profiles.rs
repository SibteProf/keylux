//! Saved lighting presets and foreground-application matching.
//!
//! Windows matches executable names or full paths; Linux/X11 matches WM_CLASS.
//! Detection runs on the render worker, including in the tray.

use std::collections::BTreeMap;

use aula_effects::registry::{Registry, Source};
use aula_effects::{ParamKind, ParamSpec, Params, Value};
use aula_protocol::Rgb;
use serde::{Deserialize, Serialize};

#[cfg(any(target_os = "linux", test))]
mod x11;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SavedValue {
    Float(f32),
    Int(i64),
    Bool(bool),
    Color([u8; 3]),
    Text(String),
    Choice(usize),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LightingPreset {
    /// Registry identity, so adding/deleting scripts cannot change the effect.
    pub effect: String,
    pub params: BTreeMap<String, SavedValue>,
}

impl Default for LightingPreset {
    fn default() -> Self {
        Self {
            effect: "solid".into(),
            params: BTreeMap::new(),
        }
    }
}

impl LightingPreset {
    pub fn capture(effect: String, params: &Params) -> Self {
        let params = params
            .iter()
            .map(|(id, value)| {
                let value = match value {
                    Value::Float(v) => SavedValue::Float(*v),
                    Value::Int(v) => SavedValue::Int(*v),
                    Value::Bool(v) => SavedValue::Bool(*v),
                    Value::Color(v) => SavedValue::Color([v.r, v.g, v.b]),
                    Value::Text(v) => SavedValue::Text(v.clone()),
                    Value::Choice(v) => SavedValue::Choice(*v),
                };
                (id.clone(), value)
            })
            .collect();
        Self { effect, params }
    }

    /// Scripts can change their declarations; ignore stale keys/types and
    /// clamp old values to the current bounds rather than trusting settings JSON.
    pub fn restore(&self, specs: &[ParamSpec]) -> Params {
        let mut params = Params::from_specs(specs);
        for spec in specs {
            let Some(saved) = self.params.get(&spec.id) else {
                continue;
            };
            let value = match (&spec.kind, saved) {
                (ParamKind::Float { min, max, .. }, SavedValue::Float(v)) if v.is_finite() => {
                    Value::Float(v.clamp(*min, *max))
                }
                (ParamKind::Int { min, max, .. }, SavedValue::Int(v)) => {
                    Value::Int((*v).clamp(*min, *max))
                }
                (ParamKind::Bool { .. }, SavedValue::Bool(v)) => Value::Bool(*v),
                (ParamKind::Color { .. }, SavedValue::Color([r, g, b])) => {
                    Value::Color(Rgb::new(*r, *g, *b))
                }
                (ParamKind::Text { .. }, SavedValue::Text(v)) => Value::Text(v.clone()),
                (ParamKind::Choice { options, .. }, SavedValue::Choice(v))
                    if *v < options.len() =>
                {
                    Value::Choice(*v)
                }
                (ParamKind::Choice { options, .. }, SavedValue::Int(v))
                    if *v >= 0 && (*v as usize) < options.len() =>
                {
                    Value::Choice(*v as usize)
                }
                _ => continue,
            };
            params.set(&spec.id, value);
        }
        params
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AppProfile {
    pub name: String,
    /// Executables/paths on Windows, WM_CLASS on X11. Comma-separated exact matches.
    pub applications: String,
    pub enabled: bool,
    pub lighting: LightingPreset,
}

impl Default for AppProfile {
    fn default() -> Self {
        Self {
            name: "New profile".into(),
            applications: String::new(),
            enabled: true,
            lighting: LightingPreset::default(),
        }
    }
}

impl AppProfile {
    fn matches(&self, application: &str) -> bool {
        self.enabled
            && self.applications.split(',').any(|pattern| {
                #[cfg(target_os = "linux")]
                {
                    let pattern = pattern.trim();
                    !pattern.is_empty() && pattern.eq_ignore_ascii_case(application.trim())
                }
                #[cfg(not(target_os = "linux"))]
                {
                    let pattern = normalize(pattern);
                    !pattern.is_empty()
                        && if pattern.contains('/') {
                            pattern == normalize(application)
                        } else {
                            pattern == executable_name(application)
                        }
                }
            })
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AppLighting {
    pub enabled: bool,
    pub default: LightingPreset,
    /// First matching enabled profile wins; order is visible/editable in the UI.
    pub profiles: Vec<AppProfile>,
}

pub fn executable_name(path: &str) -> String {
    normalize(path)
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .into()
}

fn normalize(path: &str) -> String {
    path.trim()
        .trim_matches('"')
        .replace('\\', "/")
        .to_lowercase()
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProfileStatus {
    pub application: Option<String>,
    pub active: Option<String>,
    pub warning: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Selection {
    pub index: usize,
    pub preset: LightingPreset,
    pub profile: Option<usize>,
}

/// Resolve by identity every time, including after registry reload/reconnect.
/// A missing or wireless-blocked effect falls back without erasing the profile.
pub fn resolve(
    config: &AppLighting,
    reg: &Registry,
    application: Option<&str>,
    wireless: bool,
) -> Option<(Selection, ProfileStatus)> {
    let matched = config
        .enabled
        .then(|| {
            application.and_then(|app| {
                config
                    .profiles
                    .iter()
                    .position(|profile| profile.matches(app))
            })
        })
        .flatten();
    let wanted = matched
        .map(|i| &config.profiles[i].lighting)
        .unwrap_or(&config.default);
    let usable = |preset: &LightingPreset| {
        reg.find(&preset.effect).filter(|i| {
            !wireless
                || !matches!(
                    reg.entries[*i].source,
                    Source::Animation(_) | Source::Composition(_)
                )
        })
    };
    let mut status = ProfileStatus {
        application: application.map(|app| {
            if cfg!(target_os = "linux") {
                app.to_owned()
            } else {
                executable_name(app)
            }
        }),
        active: matched.map(|i| config.profiles[i].name.clone()),
        warning: None,
    };
    let (index, preset) = if let Some(index) = usable(wanted) {
        (index, wanted.clone())
    } else {
        status.warning = Some(if reg.find(&wanted.effect).is_none() {
            format!(
                "Effect '{}' is unavailable; using default lighting.",
                wanted.effect
            )
        } else {
            "This profile needs a wired connection; using default lighting.".into()
        });
        status.active = None;
        if let Some(index) = usable(&config.default) {
            (index, config.default.clone())
        } else {
            let index = reg.find("solid")?;
            (index, LightingPreset::default())
        }
    };
    Some((
        Selection {
            index,
            preset,
            profile: matched,
        },
        status,
    ))
}

pub fn supported() -> bool {
    #[cfg(target_os = "windows")]
    {
        true
    }
    #[cfg(target_os = "linux")]
    {
        x11::available()
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        false
    }
}

#[derive(Default)]
pub struct ForegroundDetector {
    #[cfg(target_os = "linux")]
    session: Option<x11::Session>,
}

impl ForegroundDetector {
    pub fn poll(&mut self) -> Foreground {
        #[cfg(target_os = "windows")]
        {
            windows_foreground()
        }
        #[cfg(target_os = "linux")]
        {
            if !supported() {
                return Foreground::Unavailable;
            }
            if self.session.is_none() {
                self.session = x11::Session::connect().ok();
            }
            match self.session.as_ref().map(x11::Session::foreground) {
                Some(Ok(foreground)) => foreground,
                _ => {
                    self.session = None;
                    Foreground::Unavailable
                }
            }
        }
        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        {
            Foreground::Unavailable
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Foreground {
    /// Retain the current lighting while the user edits profiles in keylux.
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    OwnWindow,
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    Application(String),
    /// Focus transitions and inaccessible processes should not flash the default.
    Unavailable,
}

#[cfg(target_os = "windows")]
fn windows_foreground() -> Foreground {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowThreadProcessId,
    };

    // Only query the foreground process's executable. No hooks, input capture,
    // process enumeration, or elevated access. Always close the process handle.
    unsafe {
        let window = GetForegroundWindow();
        if window.is_null() {
            return Foreground::Unavailable;
        }
        let mut pid = 0;
        GetWindowThreadProcessId(window, &mut pid);
        if pid == std::process::id() {
            return Foreground::OwnWindow;
        }
        if pid == 0 {
            return Foreground::Unavailable;
        }
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return Foreground::Unavailable;
        }
        let mut buffer = vec![0u16; 32768];
        let mut size = buffer.len() as u32;
        let ok = QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut size);
        CloseHandle(process);
        if ok == 0 {
            return Foreground::Unavailable;
        }
        // Discard a result if focus moved during the query.
        if GetForegroundWindow() != window {
            return Foreground::Unavailable;
        }
        Foreground::Application(String::from_utf16_lossy(&buffer[..size as usize]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(name: &str, applications: &str, effect: &str) -> AppProfile {
        AppProfile {
            name: name.into(),
            applications: applications.into(),
            lighting: LightingPreset {
                effect: effect.into(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn config() -> AppLighting {
        AppLighting {
            enabled: true,
            profiles: vec![
                profile("Coding", "Code.exe, cursor.exe", "solid"),
                profile("Gaming", "game.exe", "wave"),
            ],
            default: LightingPreset {
                effect: "bloom".into(),
                ..Default::default()
            },
        }
    }

    #[test]
    fn focus_changes_choose_profiles_and_restore_the_saved_default() {
        let reg = Registry::new();
        let mut config = config();
        let mut coding = Params::from_specs(&reg.entries[reg.find("solid").unwrap()].meta.params);
        coding.set("color", Value::Color(Rgb::new(10, 30, 80)));
        config.profiles[0].lighting = LightingPreset::capture("solid".into(), &coding);
        for (app, effect, name) in [
            (r"C:\Games\game.exe", "wave", Some("Gaming")),
            (r"C:\Editors\CODE.EXE", "solid", Some("Coding")),
            (r"C:\Windows\notepad.exe", "bloom", None),
            (r"C:\Games\game.exe", "wave", Some("Gaming")),
        ] {
            let app = if cfg!(target_os = "linux") {
                executable_name(app)
            } else {
                app.into()
            };
            let (selection, status) = resolve(&config, &reg, Some(&app), false).unwrap();
            assert_eq!(reg.entries[selection.index].meta.id, effect);
            assert_eq!(status.active.as_deref(), name);
            if effect == "solid" {
                let restored = selection
                    .preset
                    .restore(&reg.entries[selection.index].meta.params);
                assert_eq!(restored.color("color", Rgb::BLACK), Rgb::new(10, 30, 80));
            }
        }
    }

    #[test]
    #[cfg(not(target_os = "linux"))]
    fn matching_is_exact_case_insensitive_and_handles_multiple_apps_and_paths() {
        let mut p = profile("Coding", " , Code.EXE, cursor.exe, ", "solid");
        assert!(p.matches(r"C:\Program Files\VS Code\code.exe"));
        assert!(p.matches("/apps/CURSOR.EXE"));
        assert!(!p.matches("decode.exe"));
        assert!(!p.matches("code.exe.bak"));
        assert!(!p.matches(""));
        p.applications = r#""C:\Editors\Code.exe""#.into();
        assert!(p.matches("c:/editors/code.exe"));
        assert!(!p.matches(r"C:\Other\Code.exe"));
        p.enabled = false;
        assert!(!p.matches(r"C:\Editors\Code.exe"));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn x11_matches_the_whole_class_without_treating_it_as_a_path() {
        let p = profile("Coding", " Code, org.example/Editor ", "solid");
        assert!(p.matches("code"));
        assert!(p.matches("org.example/Editor"));
        assert!(!p.matches("decode"));
        assert!(!p.matches("/apps/Code"));
        assert!(!p.matches("Editor"));
    }

    #[test]
    fn ordered_profiles_disabled_rules_and_global_toggle_are_respected() {
        let reg = Registry::new();
        let mut config = config();
        config
            .profiles
            .push(profile("Alternative", "Code.exe", "sweep"));
        assert_eq!(
            resolve(&config, &reg, Some("code.exe"), false)
                .unwrap()
                .1
                .active
                .as_deref(),
            Some("Coding")
        );
        config.profiles[0].enabled = false;
        assert_eq!(
            resolve(&config, &reg, Some("code.exe"), false)
                .unwrap()
                .1
                .active
                .as_deref(),
            Some("Alternative")
        );
        config.profiles.swap(0, 2);
        config.profiles[2].enabled = true;
        assert_eq!(
            resolve(&config, &reg, Some("code.exe"), false)
                .unwrap()
                .1
                .active
                .as_deref(),
            Some("Alternative")
        );
        config.enabled = false;
        let (selection, status) = resolve(&config, &reg, Some("code.exe"), false).unwrap();
        assert_eq!(selection.preset.effect, "bloom");
        assert_eq!(status.active, None);
    }

    #[test]
    fn a_missing_effect_falls_back_without_replacing_its_saved_identity() {
        let reg = Registry::new();
        let mut config = config();
        config.profiles[1].lighting.effect = "removed-script".into();
        let (selection, status) = resolve(&config, &reg, Some("game.exe"), false).unwrap();
        assert_eq!(selection.preset.effect, "bloom");
        assert_eq!(status.active, None);
        assert!(status.warning.unwrap().contains("removed-script"));
        assert_eq!(config.profiles[1].lighting.effect, "removed-script");
        config.default.effect = "also-missing".into();
        assert_eq!(
            resolve(&config, &reg, Some("game.exe"), false)
                .unwrap()
                .0
                .preset
                .effect,
            "solid"
        );
    }

    #[test]
    fn registry_reordering_does_not_change_the_saved_effect() {
        let mut reg = Registry::new();
        let config = config();
        let before = resolve(&config, &reg, Some("game.exe"), false).unwrap().0;
        reg.entries.reverse();
        let after = resolve(&config, &reg, Some("game.exe"), false).unwrap().0;
        assert_ne!(before.index, after.index);
        assert_eq!(reg.entries[after.index].meta.id, "wave");
        assert_eq!(after.preset, before.preset);
    }

    #[test]
    fn wireless_falls_back_and_wired_restores_the_animation() {
        let mut reg = Registry::new();
        let wave = reg.find("wave").unwrap();
        // Resolution uses the source kind; a real file/device is unnecessary.
        reg.entries[wave].source = Source::Composition("gaming.klx".into());
        let mut config = config();
        let (selection, status) = resolve(&config, &reg, Some("game.exe"), true).unwrap();
        assert_eq!(selection.preset.effect, "bloom");
        assert!(status.warning.unwrap().contains("wired"));
        let (selection, status) = resolve(&config, &reg, Some("game.exe"), false).unwrap();
        assert_eq!(selection.preset.effect, "wave");
        assert_eq!(status.active.as_deref(), Some("Gaming"));
        config.default.effect = "wave".into();
        assert_eq!(
            resolve(&config, &reg, Some("game.exe"), true)
                .unwrap()
                .0
                .preset
                .effect,
            "solid"
        );
    }

    #[test]
    fn settings_round_trip_every_parameter_kind() {
        let specs = vec![
            ParamSpec::float("speed", "Speed", 0.0, 5.0, 1.0),
            ParamSpec::int("count", "Count", 1, 20, 2),
            ParamSpec::boolean("reverse", "Reverse", false),
            ParamSpec::color("tint", "Tint", Rgb::BLACK),
            ParamSpec::text("message", "Message", ""),
            ParamSpec {
                id: "direction".into(),
                label: "Direction".into(),
                kind: ParamKind::Choice {
                    options: vec!["Left".into(), "Right".into()],
                    default: 0,
                },
            },
        ];
        let mut params = Params::from_specs(&specs);
        params.set("speed", Value::Float(2.5));
        params.set("count", Value::Int(12));
        params.set("reverse", Value::Bool(true));
        params.set("tint", Value::Color(Rgb::new(7, 90, 220)));
        params.set("message", Value::Text("Hello 🌈".into()));
        params.set("direction", Value::Choice(1));
        let mut settings = crate::settings::Settings {
            app_lighting: config(),
            ..Default::default()
        };
        settings.app_lighting.profiles[0].lighting =
            LightingPreset::capture("solid".into(), &params);
        let saved = serde_json::to_string(&settings).unwrap();
        let back: crate::settings::Settings = serde_json::from_str(&saved).unwrap();
        assert!(back.app_lighting.enabled);
        let restored = back.app_lighting.profiles[0].lighting.restore(&specs);
        for (id, value) in params.iter() {
            assert_eq!(restored.get(id), Some(value));
        }
        assert_eq!(restored.int("direction", 0), 1);
    }

    #[test]
    fn old_settings_keep_auto_switching_off() {
        let settings: crate::settings::Settings =
            serde_json::from_str(r#"{"max_fps":12}"#).unwrap();
        assert!(!settings.app_lighting.enabled);
        assert!(settings.app_lighting.profiles.is_empty());
        assert_eq!(settings.app_lighting.default.effect, "solid");
        assert_eq!(settings.max_fps, 12);
    }

    #[test]
    fn changed_parameter_schemas_clamp_numbers_and_ignore_stale_types() {
        let specs = vec![
            ParamSpec::float("speed", "Speed", 0.0, 5.0, 1.0),
            ParamSpec::int("count", "Count", 1, 10, 2),
            ParamSpec::boolean("reverse", "Reverse", false),
            ParamSpec {
                id: "direction".into(),
                label: "Direction".into(),
                kind: ParamKind::Choice {
                    options: vec!["Left".into(), "Right".into()],
                    default: 0,
                },
            },
        ];
        let mut preset = LightingPreset::default();
        preset
            .params
            .insert("speed".into(), SavedValue::Float(99.0));
        preset.params.insert("count".into(), SavedValue::Int(-20));
        preset
            .params
            .insert("reverse".into(), SavedValue::Text("wrong type".into()));
        preset
            .params
            .insert("direction".into(), SavedValue::Choice(500));
        preset
            .params
            .insert("deleted".into(), SavedValue::Bool(true));
        let restored = preset.restore(&specs);
        assert_eq!(restored.float("speed", 0.0), 5.0);
        assert_eq!(restored.int("count", 0), 1);
        assert!(!restored.bool("reverse", true));
        assert_eq!(restored.int("direction", -1), 0);
        assert!(restored.get("deleted").is_none());
        preset
            .params
            .insert("speed".into(), SavedValue::Float(f32::NAN));
        assert_eq!(preset.restore(&specs).float("speed", 0.0), 1.0);
        preset.params.insert("direction".into(), SavedValue::Int(1));
        assert_eq!(preset.restore(&specs).int("direction", 0), 1);
    }
}
