//! Effect parameters.
//!
//! Effects declare their knobs; the GUI renders controls from the declaration
//! and stores values here. This is the whole extensibility story: a script that
//! declares a `float` param gets a slider, a `color` param gets a colour
//! picker, and no UI code changes.

use std::collections::HashMap;

use aula_protocol::Rgb;

/// The kind of control a parameter needs, plus its bounds and default.
#[derive(Clone, Debug, PartialEq)]
pub enum ParamKind {
    Float {
        min: f32,
        max: f32,
        default: f32,
    },
    Int {
        min: i64,
        max: i64,
        default: i64,
    },
    Bool {
        default: bool,
    },
    Color {
        default: Rgb,
    },
    /// Free text, e.g. the message for a scrolling-text effect.
    Text {
        default: String,
    },
    /// One of a fixed set.
    Choice {
        options: Vec<String>,
        default: usize,
    },
}

/// One declared parameter.
#[derive(Clone, Debug, PartialEq)]
pub struct ParamSpec {
    /// Stable key used to look the value up.
    pub id: String,
    /// Label shown in the UI.
    pub label: String,
    pub kind: ParamKind,
}

impl ParamSpec {
    pub fn float(id: &str, label: &str, min: f32, max: f32, default: f32) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            kind: ParamKind::Float { min, max, default },
        }
    }

    pub fn int(id: &str, label: &str, min: i64, max: i64, default: i64) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            kind: ParamKind::Int { min, max, default },
        }
    }

    pub fn boolean(id: &str, label: &str, default: bool) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            kind: ParamKind::Bool { default },
        }
    }

    pub fn color(id: &str, label: &str, default: Rgb) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            kind: ParamKind::Color { default },
        }
    }

    pub fn text(id: &str, label: &str, default: &str) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            kind: ParamKind::Text {
                default: default.into(),
            },
        }
    }
}

/// A parameter value.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Float(f32),
    Int(i64),
    Bool(bool),
    Color(Rgb),
    Text(String),
    Choice(usize),
}

/// Current values for one effect, keyed by `ParamSpec::id`.
///
/// Lookups never fail: an unknown or mistyped key returns the caller's
/// fallback. Effects are frequently user scripts, and a typo should degrade
/// gracefully rather than panic mid-animation.
#[derive(Clone, Debug, Default)]
pub struct Params {
    values: HashMap<String, Value>,
}

impl Params {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a value set from an effect's declared defaults.
    pub fn from_specs(specs: &[ParamSpec]) -> Self {
        let mut values = HashMap::new();
        for s in specs {
            let v = match &s.kind {
                ParamKind::Float { default, .. } => Value::Float(*default),
                ParamKind::Int { default, .. } => Value::Int(*default),
                ParamKind::Bool { default } => Value::Bool(*default),
                ParamKind::Color { default } => Value::Color(*default),
                ParamKind::Text { default } => Value::Text(default.clone()),
                ParamKind::Choice { default, .. } => Value::Choice(*default),
            };
            values.insert(s.id.clone(), v);
        }
        Self { values }
    }

    pub fn set(&mut self, id: &str, v: Value) {
        self.values.insert(id.to_string(), v);
    }

    pub fn get(&self, id: &str) -> Option<&Value> {
        self.values.get(id)
    }

    pub fn float(&self, id: &str, fallback: f32) -> f32 {
        match self.values.get(id) {
            Some(Value::Float(v)) => *v,
            Some(Value::Int(v)) => *v as f32,
            _ => fallback,
        }
    }

    pub fn int(&self, id: &str, fallback: i64) -> i64 {
        match self.values.get(id) {
            Some(Value::Int(v)) => *v,
            Some(Value::Float(v)) => *v as i64,
            _ => fallback,
        }
    }

    pub fn bool(&self, id: &str, fallback: bool) -> bool {
        match self.values.get(id) {
            Some(Value::Bool(v)) => *v,
            _ => fallback,
        }
    }

    pub fn color(&self, id: &str, fallback: Rgb) -> Rgb {
        match self.values.get(id) {
            Some(Value::Color(v)) => *v,
            _ => fallback,
        }
    }

    pub fn text(&self, id: &str, fallback: &str) -> String {
        match self.values.get(id) {
            Some(Value::Text(v)) => v.clone(),
            _ => fallback.to_string(),
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &Value)> {
        self.values.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_materialised() {
        let specs = vec![
            ParamSpec::float("speed", "Speed", 0.0, 5.0, 1.5),
            ParamSpec::color("tint", "Tint", Rgb::new(1, 2, 3)),
        ];
        let p = Params::from_specs(&specs);
        assert_eq!(p.float("speed", 0.0), 1.5);
        assert_eq!(p.color("tint", Rgb::BLACK), Rgb::new(1, 2, 3));
    }

    #[test]
    fn missing_and_mistyped_keys_fall_back() {
        let mut p = Params::new();
        p.set("speed", Value::Text("oops".into()));
        assert_eq!(p.float("speed", 9.0), 9.0, "wrong type falls back");
        assert_eq!(p.float("absent", 4.0), 4.0, "missing key falls back");
    }

    #[test]
    fn int_and_float_coerce() {
        let mut p = Params::new();
        p.set("n", Value::Int(3));
        assert_eq!(p.float("n", 0.0), 3.0);
        p.set("f", Value::Float(2.7));
        assert_eq!(p.int("f", 0), 2);
    }
}
