//! Old onboard commands must fail before device discovery or configuration access.
use std::path::PathBuf;
use std::process::Command;

#[test]
fn withdrawn_onboard_commands_report_disabled_controls() {
    let binary = std::env::var_os("KEYLUX_TEST_BINARY")
        .map(PathBuf::from)
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_keylux").into());
    for action in ["list", "status", "apply"] {
        let output = Command::new(&binary)
            .args(["onboard", action, "3", "--native", "--device", "3554:fa09"])
            .output()
            .unwrap();
        assert!(!output.status.success(), "{action} unexpectedly succeeded");
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(
            error.contains("Onboard lighting controls are disabled"),
            "{action}: {error}"
        );
        assert!(output.stdout.is_empty());
    }
}
