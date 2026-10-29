//! Reported hardware variants whose lighting protocol has not been verified.
use crate::DeviceId;

/// Reported in issue #17; not confirmed to understand Sinowealth F75 commands.
pub const F75_WIRED_UNVERIFIED: DeviceId = DeviceId::new(0x1a2c, 0x9407);

pub fn unsupported_reason(id: DeviceId) -> Option<&'static str> {
    (id == F75_WIRED_UNVERIFIED).then_some(
        "AULA F75 Wired variant (1a2c:9407) detected. Its lighting protocol is not yet supported; \
         per-key colors and animations are unavailable. Run `keylux diagnose --device 1a2c:9407` \
         and see docs/F75-WIRED.md (issue #17).",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reported_variant_is_distinct_from_supported_hardware() {
        assert!(unsupported_reason(F75_WIRED_UNVERIFIED).is_some());
        assert!(unsupported_reason(crate::f75::protocol::WIRED).is_none());
        assert!(unsupported_reason(DeviceId::new(0x1a2c, 0x9408)).is_none());
    }
}
