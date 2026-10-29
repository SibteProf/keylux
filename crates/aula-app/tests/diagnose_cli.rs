use std::process::Command;

fn binary() -> std::path::PathBuf {
    std::env::var_os("KEYLUX_TEST_BINARY")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_keylux").into())
}

#[test]
fn diagnostics_can_be_saved_from_the_packaged_binary() {
    let path = std::env::temp_dir().join(format!("keylux-diagnostics-{}.txt", std::process::id()));
    let result = Command::new(binary())
        .args(["diagnose", "--device", "1a2c:9407", "--output"])
        .arg(&path)
        .output()
        .unwrap();
    let report = std::fs::read_to_string(&path).unwrap();
    std::fs::remove_file(path).unwrap();
    assert!(
        report.contains("Enumeration only: no device handles opened; no protocol commands sent.")
    );
    assert!(report.contains("HID paths and serial numbers are omitted."));
    if report.contains("0 HID collection(s)") {
        assert!(!result.status.success());
    } else {
        assert!(result.status.success());
        assert!(report.contains("lighting protocol is not yet supported"));
    }
}

#[test]
fn diagnostics_reject_invalid_or_missing_device_ids() {
    for args in [
        vec!["diagnose", "--device"],
        vec!["diagnose", "--device", "xyz"],
    ] {
        assert!(!Command::new(binary())
            .args(args)
            .output()
            .unwrap()
            .status
            .success());
    }
}
