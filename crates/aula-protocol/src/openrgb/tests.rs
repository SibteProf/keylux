use super::*;
use crate::Rgb;
use std::net::TcpListener;
use std::thread;

fn u16_bytes(out: &mut Vec<u8>, value: usize) {
    out.extend_from_slice(&(value as u16).to_le_bytes());
}
fn u32_bytes(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}
fn string(out: &mut Vec<u8>, value: &str) {
    u16_bytes(out, value.len() + 1);
    out.extend_from_slice(value.as_bytes());
    out.push(0);
}

// Independent SDK v3 fixture: 3x2 keyboard matrix with a gap and a repeated
// wide key, followed by a two-LED accent zone. Vendor LED values deliberately
// differ from frame indices. Uses every v3 mode field (including brightness).
fn fixture(name: &str, serial: &str) -> Vec<u8> {
    let mut b = vec![0; 4];
    u32_bytes(&mut b, 5);
    for s in [
        name,
        "Test vendor",
        "Fixture keyboard",
        "1.0",
        serial,
        "USB: port 2",
    ] {
        string(&mut b, s);
    }
    u16_bytes(&mut b, 2);
    u32_bytes(&mut b, 0);
    for (name, flags, color_mode) in [("Direct", 32, 1), ("Rainbow", 1, 0)] {
        string(&mut b, name);
        for value in [7, flags, 1, 10, 0, 100, 0, 0, 3, 50, 0, color_mode] {
            u32_bytes(&mut b, value);
        }
        u16_bytes(&mut b, 0);
    }
    u16_bytes(&mut b, 2);
    string(&mut b, "Keys");
    for value in [2, 4, 4, 4] {
        u32_bytes(&mut b, value);
    }
    u16_bytes(&mut b, 8 + 6 * 4);
    for value in [2, 3, 2, u32::MAX, 0, 1, 1, 3] {
        u32_bytes(&mut b, value);
    }
    string(&mut b, "Accent");
    for value in [1, 2, 2, 2] {
        u32_bytes(&mut b, value);
    }
    u16_bytes(&mut b, 0);
    u16_bytes(&mut b, 6);
    for (i, s) in ["A", "Space", "Esc", "Enter", "Accent 1", "Accent 2"]
        .iter()
        .enumerate()
    {
        string(&mut b, s);
        u32_bytes(&mut b, (100 + i) as u32);
    }
    u16_bytes(&mut b, 6);
    b.extend_from_slice(&[0; 6 * 4]);
    let len = b.len() as u32;
    b[..4].copy_from_slice(&len.to_le_bytes());
    b
}

fn receive(socket: &mut TcpStream) -> (u32, u32, Vec<u8>) {
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut header = [0; 16];
    socket.read_exact(&mut header).unwrap();
    assert_eq!(&header[..4], b"ORGB");
    let dev = u32::from_le_bytes(header[4..8].try_into().unwrap());
    let id = u32::from_le_bytes(header[8..12].try_into().unwrap());
    let len = u32::from_le_bytes(header[12..16].try_into().unwrap());
    let mut data = vec![0; len as usize];
    socket.read_exact(&mut data).unwrap();
    (dev, id, data)
}

fn respond(socket: &mut TcpStream, dev: u32, id: u32, data: &[u8]) {
    // Fragment header and body to exercise TCP stream framing.
    let mut packet = b"ORGB".to_vec();
    for n in [dev, id, data.len() as u32] {
        u32_bytes(&mut packet, n);
    }
    packet.extend_from_slice(data);
    for chunk in packet.chunks(7) {
        socket.write_all(chunk).unwrap();
    }
}

fn handshake(socket: &mut TcpStream) {
    assert_eq!(receive(socket), (0, 40, 3u32.to_le_bytes().to_vec()));
    // A newer server must receive v3 controller requests after negotiation.
    respond(socket, 0, 40, &6u32.to_le_bytes());
    assert_eq!(receive(socket), (0, 50, b"keylux\0".to_vec()));
}

fn open_fixture(socket: &mut TcpStream, data: &[u8]) {
    handshake(socket);
    assert_eq!(receive(socket), (0, 0, vec![]));
    respond(socket, 0, 0, &1u32.to_le_bytes());
    expect_controller(socket, data);
}

fn expect_controller(socket: &mut TcpStream, data: &[u8]) {
    assert_eq!(receive(socket), (0, 1, 3u32.to_le_bytes().to_vec()));
    respond(socket, 0, 1, data);
}

#[test]
fn parses_geometry_in_zone_order_not_vendor_led_values() {
    let c = parse_controller(7, &fixture("Test keyboard", "serial")).unwrap();
    assert_eq!(c.index, 7);
    assert!(c.is_keyboard() && c.supports_direct() && c.has_matrix);
    assert_eq!(c.led_count, 6);
    assert_eq!(c.layout.len(), 6);
    let esc = c.layout.iter().find(|k| k.name == "Esc").unwrap();
    assert_eq!((esc.led, esc.row, esc.x), (2, 0, 0.5));
    let space = c.layout.iter().find(|k| k.name == "Space").unwrap();
    assert_eq!((space.led, space.row, space.w), (1, 1, 2.0));
    assert_eq!(c.layout.iter().find(|k| k.led == 4).unwrap().row, 2);
}

#[test]
fn identities_do_not_depend_on_controller_index_and_distinguish_copies() {
    let a = parse_controller(0, &fixture("Same", "one")).unwrap();
    let b = parse_controller(9, &fixture("Same", "one")).unwrap();
    let c = parse_controller(0, &fixture("Same", "two")).unwrap();
    assert_eq!(a.key(), b.key());
    assert_ne!(a.key(), c.key());
    assert!(a.same_target(&b));
    assert!(!a.same_target(&c));
}

#[test]
fn selectors_are_shell_safe_and_preserve_field_boundaries() {
    let mut a = parse_controller(0, &fixture("Keyboard \"日本語\"", "USB: back\\slash")).unwrap();
    let selector = a.key();
    assert!(selector
        .strip_prefix("openrgb:")
        .unwrap()
        .chars()
        .all(|c| c.is_ascii_hexdigit() || c == ':'));
    a.name = "ab".into();
    a.vendor = "c".into();
    let first = a.key();
    a.name = "a".into();
    a.vendor = "bc".into();
    assert_ne!(first, a.key());
}

#[test]
fn all_truncated_packets_and_incorrect_lengths_are_rejected() {
    let data = fixture("Test", "");
    for end in 0..data.len() {
        let mut truncated = data[..end].to_vec();
        if end >= 4 {
            truncated[..4].copy_from_slice(&(end as u32).to_le_bytes());
        }
        assert!(
            parse_controller(0, &truncated).is_err(),
            "accepted {end} bytes"
        );
    }
    let mut longer = data.clone();
    longer.push(0);
    assert!(parse_controller(0, &longer).is_err());
    let len = longer.len() as u32;
    longer[..4].copy_from_slice(&len.to_le_bytes());
    assert!(parse_controller(0, &longer).is_err());
}

#[test]
fn invalid_matrix_indices_and_dimensions_are_rejected() {
    let data = fixture("Test", "");
    let needle: Vec<_> = [2u32, 3, 2, u32::MAX, 0, 1, 1, 3]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    let at = data
        .windows(needle.len())
        .position(|w| w == needle)
        .unwrap();
    for (offset, value) in [(0, u32::MAX), (4, 0), (8, 4)] {
        let mut bad = data.clone();
        bad[at + offset..at + offset + 4].copy_from_slice(&value.to_le_bytes());
        assert!(parse_controller(0, &bad).is_err());
    }
}

#[test]
fn matrix_indices_are_relative_to_each_zone() {
    let zones = vec![
        Zone {
            count: 2,
            width: 0,
            height: 0,
            map: vec![],
        },
        Zone {
            count: 2,
            width: 2,
            height: 1,
            map: vec![1, 0],
        },
    ];
    let names = ["Logo 1", "Logo 2", "A", "B"].map(str::to_string);
    let (layout, physical) = make_layout(&zones, &names).unwrap();
    assert!(physical);
    assert_eq!((layout[0].name.as_str(), layout[0].led), ("B", 3));
    assert_eq!((layout[1].name.as_str(), layout[1].led), ("A", 2));
    assert_eq!(layout[2].row, 1);
}

#[test]
fn missing_matrix_is_a_complete_led_grid() {
    let names: Vec<_> = (0..104).map(|n| format!("LED {n}")).collect();
    let (layout, physical) = make_layout(&[], &names).unwrap();
    assert!(!physical);
    assert_eq!(layout.len(), 104);
    assert_eq!(layout[103].led, 103);
    assert_eq!(layout[103].row, 4);
}

#[test]
fn discovery_is_read_only_and_stream_uses_rgbx_with_exact_size() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let data = fixture("Test keyboard", "serial");
    let key = parse_controller(0, &data).unwrap().key();
    let server = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        open_fixture(&mut socket, &data);
        // ensure mode: verify, set custom, then verify its result.
        expect_controller(&mut socket, &data);
        assert_eq!(receive(&mut socket), (0, 1100, vec![]));
        expect_controller(&mut socket, &data);
        expect_controller(&mut socket, &data);
        let mut expected = vec![30, 0, 0, 0, 6, 0];
        expected.extend_from_slice(&[1, 2, 3, 0, 20, 40, 60, 0]);
        expected.extend_from_slice(&[0; 16]);
        assert_eq!(receive(&mut socket), (0, 1050, expected));
        // Even an unchanged frame checks the identity, but sends no RGB write.
        expect_controller(&mut socket, &data);
        let mut byte = [0];
        assert_eq!(socket.read(&mut byte).unwrap(), 0);
    });
    let mut kb = OpenRgbKeyboard::open(addr, &key).unwrap();
    assert!(kb.set_static(&Frame::black(6)).is_err());
    assert!(kb.stream(&Frame::black(6)).is_err());
    kb.ensure_per_key_mode().unwrap();
    assert!(kb.stream(&Frame::black(5)).is_err());
    let mut frame = Frame::black(6);
    frame.set(0, Rgb::new(1, 2, 3));
    frame.set(1, Rgb::new(20, 40, 60));
    kb.stream(&frame).unwrap();
    kb.stream(&frame).unwrap();
    kb.set_max_fps(500);
    assert_eq!(kb.max_fps(), MAX_FPS);
    kb.set_max_fps(0);
    assert_eq!(kb.max_fps(), 1);
    drop(kb);
    server.join().unwrap();
}

#[test]
fn index_reuse_and_hotplug_stop_writes_and_invalidate_connection() {
    for hotplug in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let data = fixture("Original", "one");
        let key = parse_controller(0, &data).unwrap().key();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            open_fixture(&mut socket, &data);
            assert_eq!(receive(&mut socket), (0, 1, 3u32.to_le_bytes().to_vec()));
            if hotplug {
                respond(&mut socket, 0, 100, &[]);
            } else {
                respond(&mut socket, 0, 1, &fixture("Replacement", "two"));
            }
            let mut byte = [0];
            assert_eq!(
                socket.read(&mut byte).unwrap(),
                0,
                "must not send writes or retry on an invalidated stream"
            );
        });
        let mut kb = OpenRgbKeyboard::open(addr, &key).unwrap();
        assert!(kb.ensure_per_key_mode().is_err());
        assert!(kb.ensure_per_key_mode().is_err());
        drop(kb);
        server.join().unwrap();
    }
}

#[test]
fn malformed_headers_and_old_servers_fail_closed() {
    for case in 0..4 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            receive(&mut socket);
            match case {
                0 => respond(&mut socket, 0, 40, &2u32.to_le_bytes()),
                1 => respond(&mut socket, 1, 40, &3u32.to_le_bytes()),
                2 => {
                    socket
                        .write_all(b"BAD!\0\0\0\0\x28\0\0\0\x04\0\0\0")
                        .unwrap();
                }
                _ => {
                    let mut header = b"ORGB\0\0\0\0\x28\0\0\0".to_vec();
                    header.extend_from_slice(&((MAX_PACKET + 1) as u32).to_le_bytes());
                    socket.write_all(&header).unwrap();
                }
            }
        });
        assert!(Client::connect(addr).is_err());
        server.join().unwrap();
    }
}

#[test]
fn non_keyboard_controllers_are_skipped_without_parsing_their_zones() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        handshake(&mut socket);
        assert_eq!(receive(&mut socket), (0, 0, vec![]));
        respond(&mut socket, 0, 0, &2u32.to_le_bytes());
        assert_eq!(receive(&mut socket), (0, 1, 3u32.to_le_bytes().to_vec()));
        // Only the size and type are relevant for unrelated controllers.
        respond(&mut socket, 0, 1, &[8, 0, 0, 0, 2, 0, 0, 0]);
        assert_eq!(receive(&mut socket), (1, 1, 3u32.to_le_bytes().to_vec()));
        respond(&mut socket, 1, 1, &fixture("Keyboard", "one"));
    });
    let keyboards = Client::connect(addr).unwrap().controllers().unwrap();
    assert_eq!(keyboards.len(), 1);
    assert_eq!(keyboards[0].index, 1);
    server.join().unwrap();
}

#[test]
fn ambiguous_identities_and_missing_direct_modes_are_rejected_without_writes() {
    for ambiguous in [true, false] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let mut data = fixture("Keyboard", "same");
        if !ambiguous {
            let at = data.windows(6).position(|s| s == b"Direct").unwrap();
            data[at..at + 6].copy_from_slice(b"Static");
        }
        let key = parse_controller(0, &data).unwrap().key();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            handshake(&mut socket);
            assert_eq!(receive(&mut socket), (0, 0, vec![]));
            let count = if ambiguous { 2u32 } else { 1 };
            respond(&mut socket, 0, 0, &count.to_le_bytes());
            for index in 0..count {
                assert_eq!(
                    receive(&mut socket),
                    (index, 1, 3u32.to_le_bytes().to_vec())
                );
                respond(&mut socket, index, 1, &data);
            }
            let mut byte = [0];
            assert_eq!(socket.read(&mut byte).unwrap(), 0);
        });
        assert!(OpenRgbKeyboard::open(addr, &key).is_err());
        server.join().unwrap();
    }
}
