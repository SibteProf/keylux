//! Exercise the actual CLI binary against a loopback SDK server. The explicit
//! --openrgb option restricts discovery to this server; no HID access occurs.
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::thread;
use std::time::Duration;

fn put_string(data: &mut Vec<u8>, text: &str) {
    data.extend_from_slice(&((text.len() + 1) as u16).to_le_bytes());
    data.extend_from_slice(text.as_bytes());
    data.push(0);
}

fn description(name: &str) -> Vec<u8> {
    let mut data = vec![0; 4];
    data.extend_from_slice(&5u32.to_le_bytes());
    for text in [
        name,
        "Test",
        "SDK fixture",
        "1",
        "serial-123",
        r"HID:\\?\vid_1234&pid_5678#port",
    ] {
        put_string(&mut data, text);
    }
    data.extend_from_slice(&1u16.to_le_bytes()); // one Direct mode
    data.extend_from_slice(&0u32.to_le_bytes());
    put_string(&mut data, "Direct");
    for value in [0u32, 32, 0, 0, 0, 100, 0, 0, 0, 100, 0, 1] {
        data.extend_from_slice(&value.to_le_bytes());
    }
    data.extend_from_slice(&0u16.to_le_bytes()); // no mode-specific colors
    data.extend_from_slice(&1u16.to_le_bytes()); // one zone
    put_string(&mut data, "Keys");
    for value in [2u32, 2, 2, 2] {
        data.extend_from_slice(&value.to_le_bytes());
    }
    data.extend_from_slice(&16u16.to_le_bytes()); // matrix byte size
    for value in [1u32, 2, 1, 0] {
        data.extend_from_slice(&value.to_le_bytes());
    }
    data.extend_from_slice(&2u16.to_le_bytes());
    for name in ["A", "B"] {
        put_string(&mut data, name);
        data.extend_from_slice(&99u32.to_le_bytes());
    }
    data.extend_from_slice(&2u16.to_le_bytes());
    data.extend_from_slice(&[0; 8]);
    let size = data.len() as u32;
    data[..4].copy_from_slice(&size.to_le_bytes());
    data
}

fn serve(mut stream: TcpStream, description: &[u8], allow_writes: bool) -> usize {
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut frames = 0;
    let mut direct = false;
    loop {
        let mut header = [0; 16];
        match stream.read_exact(&mut header) {
            Ok(()) => {}
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::ConnectionReset
                ) =>
            {
                break
            }
            Err(e) => panic!("client did not finish: {e}"),
        }
        assert_eq!(&header[..4], b"ORGB");
        assert_eq!(&header[4..8], &[0; 4]);
        let id = u32::from_le_bytes(header[8..12].try_into().unwrap());
        let size = u32::from_le_bytes(header[12..16].try_into().unwrap());
        let mut body = vec![0; size as usize];
        stream.read_exact(&mut body).unwrap();
        let response = match id {
            40 => {
                assert_eq!(body, 3u32.to_le_bytes());
                Some(5u32.to_le_bytes().to_vec())
            }
            50 => {
                assert_eq!(body, b"keylux\0");
                None
            }
            0 => {
                assert!(body.is_empty());
                Some(1u32.to_le_bytes().to_vec())
            }
            1 => {
                assert_eq!(body, 3u32.to_le_bytes());
                Some(description.to_vec())
            }
            1100 => {
                assert!(allow_writes && body.is_empty());
                direct = true;
                None
            }
            1050 => {
                assert!(allow_writes && direct);
                // The full CLI applies --color through the effect engine.
                assert_eq!(body, [14, 0, 0, 0, 2, 0, 18, 52, 86, 0, 18, 52, 86, 0]);
                frames += 1;
                None
            }
            _ => panic!("unexpected packet {id}; no EEPROM commands are permitted"),
        };
        if let Some(response) = response {
            header[12..16].copy_from_slice(&(response.len() as u32).to_le_bytes());
            stream.write_all(&header).unwrap();
            stream.write_all(&response).unwrap();
        }
    }
    frames
}

fn cli() -> Command {
    // Also exercise the release/archive binary without changing the fixtures.
    let binary = std::env::var_os("KEYLUX_TEST_BINARY")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_keylux").into());
    let mut cmd = Command::new(&binary);
    if std::env::var_os("KEYLUX_TEST_BINARY").is_some() {
        cmd.current_dir(binary.parent().unwrap());
    }
    // Use an empty settings directory so the user's pinned hardware cannot
    // change test selection. These commands never save preferences.
    let empty = std::env::temp_dir().join(format!("keylux-sdk-cli-{}", std::process::id()));
    cmd.env("APPDATA", &empty).env("XDG_CONFIG_HOME", empty);
    cmd
}

#[test]
fn cli_reports_the_build_version() {
    let result = cli().arg("--version").output().unwrap();
    assert!(result.status.success());
    assert_eq!(
        String::from_utf8_lossy(&result.stdout).trim(),
        format!("keylux {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn cli_discovers_then_streams_through_the_selected_sdk_keyboard() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = listener.local_addr().unwrap().to_string();
    let server = thread::spawn(move || {
        let data = description("CLI test keyboard");
        // `devices` is read-only, then effect discovery is read-only, then open.
        for _ in 0..2 {
            let (stream, _) = listener.accept().unwrap();
            assert_eq!(serve(stream, &data, false), 0);
        }
        let (stream, _) = listener.accept().unwrap();
        assert_eq!(
            serve(stream, &data, true),
            1,
            "unchanged frames should be deduplicated"
        );
    });
    let listing = cli()
        .args(["devices", "--openrgb", &endpoint])
        .output()
        .unwrap();
    assert!(
        listing.status.success(),
        "{}",
        String::from_utf8_lossy(&listing.stderr)
    );
    let listing_text = String::from_utf8_lossy(&listing.stdout);
    assert!(listing_text.contains("CLI test keyboard"));
    let selector = listing_text
        .lines()
        .find_map(|line| line.strip_prefix("  --device "))
        .unwrap();
    assert!(selector.starts_with("openrgb:") && !selector.contains('\\'));
    let effect = cli()
        .args([
            "solid",
            "0.2",
            "--device",
            selector,
            "--openrgb",
            &endpoint,
            "--fps",
            "10",
            "--color",
            "#123456",
        ])
        .output()
        .unwrap();
    assert!(
        effect.status.success(),
        "{}",
        String::from_utf8_lossy(&effect.stderr)
    );
    assert!(String::from_utf8_lossy(&effect.stdout).contains("Stopped."));
    server.join().unwrap();
}

#[test]
fn cli_reports_an_unavailable_sdk_server_without_panicking() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = listener.local_addr().unwrap().to_string();
    drop(listener);
    let result = cli()
        .args(["devices", "--openrgb", &endpoint])
        .output()
        .unwrap();
    assert!(!result.status.success());
    let error = String::from_utf8_lossy(&result.stderr);
    assert!(
        error.contains("SDK") && error.contains("No compatible keyboards"),
        "{error}"
    );
}
