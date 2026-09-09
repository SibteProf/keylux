//! Decode a USBPcap capture of the AULA vendor driver.
//!
//! This is the ground-truth path: it reads what the official software actually
//! sends. The wired protocol was found this way, and it is the only way to find
//! out whether the 2.4 GHz receiver carries colours at all — probing cannot
//! answer that, because the receiver's collections are too narrow to try.
//!
//! Unlike the TypeScript `parse-capture.ts` this replaces, it reads the `.pcap`
//! straight from USBPcap (no Wireshark JSON export step) and assumes nothing
//! about report IDs or packet sizes — the dongle uses neither of the wired
//! protocol's.
//!
//! ```text
//! cargo run --example parse_capture -- capture.pcap
//! cargo run --example parse_capture -- capture.pcap --device 12   # one USB address
//! cargo run --example parse_capture -- capture.pcap --min-len 32  # only big payloads
//! cargo run --example parse_capture -- capture.pcap --all         # don't collapse repeats
//! ```

use std::collections::BTreeMap;

/// USBPcap's link type.
const DLT_USBPCAP: u32 = 249;

#[derive(Clone)]
struct Record {
    frame: usize,
    /// Seconds since the first record in the file.
    t: f64,
    bus: u16,
    device: u16,
    endpoint: u8,
    transfer: u8,
    /// True when the packet came from the device.
    from_device: bool,
    stage: Option<u8>,
    data: Vec<u8>,
}

impl Record {
    fn kind(&self) -> &'static str {
        match self.transfer {
            0 => "isochronous",
            1 => "interrupt",
            2 => "control",
            3 => "bulk",
            _ => "?",
        }
    }

    fn dir(&self) -> &'static str {
        if self.from_device {
            "IN "
        } else {
            "OUT"
        }
    }

    /// Host-to-device payloads are the interesting ones: they are the commands.
    fn is_command(&self) -> bool {
        !self.from_device && !self.data.is_empty()
    }

    /// A control transfer carries an 8-byte SETUP in front of its payload.
    /// Splitting them is the difference between reading the protocol and
    /// reading a wall of hex.
    fn setup(&self) -> Option<Setup> {
        if self.transfer != 2 || self.data.len() < 8 || self.stage != Some(0) {
            return None;
        }
        let d = &self.data;
        Some(Setup {
            request_type: d[0],
            request: d[1],
            value: u16le(d, 2),
            index: u16le(d, 4),
            length: u16le(d, 6),
        })
    }

    /// The bytes that are actually the protocol, with any SETUP stripped.
    fn payload(&self) -> &[u8] {
        if self.setup().is_some() {
            &self.data[8..]
        } else {
            &self.data
        }
    }
}

struct Setup {
    request_type: u8,
    request: u8,
    value: u16,
    index: u16,
    length: u16,
}

impl Setup {
    /// HID class requests, which is where report traffic lives.
    fn describe(&self) -> String {
        let name = match (self.request_type, self.request) {
            (0x21, 0x09) => "SET_REPORT",
            (0xa1, 0x01) => "GET_REPORT",
            (0x21, 0x0a) => "SET_IDLE",
            (0x80 | 0x81, 0x06) => "GET_DESCRIPTOR",
            _ => "",
        };
        if name.is_empty() {
            return format!(
                "bmRequestType={:#04x} bRequest={:#04x} wValue={:#06x} wLength={}",
                self.request_type, self.request, self.value, self.length
            );
        }
        let kind = match self.value >> 8 {
            1 => "input",
            2 => "output",
            3 => "feature",
            _ => "?",
        };
        format!(
            "{name} {kind} report {:#04x}, iface {}, {} bytes",
            self.value & 0xff,
            self.index,
            self.length
        )
    }
}

fn u16le(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}
fn u32le(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(path) = args.first().filter(|a| !a.starts_with("--")) else {
        eprintln!("usage: parse_capture <capture.pcap> [--device N] [--min-len N] [--all]");
        std::process::exit(1);
    };
    let flag = |name: &str| -> Option<usize> {
        let i = args.iter().position(|a| a == name)?;
        args.get(i + 1)?.parse().ok()
    };
    let only_device = flag("--device");
    let min_len = flag("--min-len").unwrap_or(0);
    let show_all = args.iter().any(|a| a == "--all");

    let bytes = std::fs::read(path)?;
    let records = parse(&bytes)?;
    // The span matters when a capture shows an ABSENCE: "no lighting traffic"
    // only means something if the capture actually covers a decent window.
    let span = records.last().map(|r| r.t).unwrap_or(0.0);
    println!(
        "{} USB records in {path}, spanning {span:.1} s\n",
        records.len()
    );

    let interesting: Vec<&Record> = records
        .iter()
        .filter(|r| r.payload().len() >= min_len)
        // Not `is_none_or`: that is Rust 1.82 and this crate targets 1.75.
        .filter(|r| match only_device {
            Some(d) => usize::from(r.device) == d,
            None => true,
        })
        .filter(|r| !r.payload().is_empty())
        .collect();

    // ---- what the bus is carrying, in one table -------------------------
    // This is the part that answers the question. If the driver drives colours
    // over this device, there will be a high-volume OUT shape here that is wide
    // enough to hold them.
    let mut shapes: BTreeMap<(u16, u8, bool, u8, usize), usize> = BTreeMap::new();
    for r in &interesting {
        *shapes
            .entry((
                r.device,
                r.transfer,
                r.from_device,
                r.payload().first().copied().unwrap_or(0),
                r.payload().len(),
            ))
            .or_default() += 1;
    }

    println!("Traffic shapes (device, kind, direction, first byte, length, count):");
    for ((dev, transfer, from_device, first, len), count) in &shapes {
        let kind = match transfer {
            0 => "isochronous",
            1 => "interrupt",
            2 => "control",
            3 => "bulk",
            _ => "?",
        };
        let dir = if *from_device { "IN " } else { "OUT" };
        println!("  dev {dev:<3} {kind:<11} {dir}  first=0x{first:02x}  len={len:<4}  x{count}");
    }

    let widest = interesting
        .iter()
        .filter(|r| r.is_command())
        .map(|r| r.payload().len())
        .max()
        .unwrap_or(0);
    println!(
        "\nWidest host-to-device payload: {widest} bytes. \
         A per-key frame needs 378 for this board;\nanything narrower means the driver chunks it."
    );

    // ---- the packets themselves -----------------------------------------
    println!("\nHost-to-device packets:");
    let mut last: Option<Vec<u8>> = None;
    let mut repeats = 0usize;
    for r in interesting.iter().filter(|r| r.is_command()) {
        if !show_all && last.as_deref() == Some(r.data.as_slice()) {
            repeats += 1;
            continue;
        }
        if repeats > 0 {
            println!("      ... and {repeats} identical repeats");
            repeats = 0;
        }
        println!(
            "  #{:<6} t={:<8.3} bus{} dev{} ep{:#04x} {:<11} {}  {}",
            r.frame,
            r.t,
            r.bus,
            r.device,
            r.endpoint,
            r.kind(),
            r.dir(),
            match r.setup() {
                Some(s) => s.describe(),
                None => format!("{} bytes", r.data.len()),
            }
        );
        let p = r.payload();
        if !p.is_empty() {
            println!("      {}", hex(p, 32));
        }
        last = Some(r.data.clone());
    }
    if repeats > 0 {
        println!("      ... and {repeats} identical repeats");
    }

    Ok(())
}

fn hex(data: &[u8], max: usize) -> String {
    let shown: Vec<String> = data.iter().take(max).map(|b| format!("{b:02x}")).collect();
    let mut s = shown.join(" ");
    if data.len() > max {
        s.push_str(&format!("  ... (+{} bytes)", data.len() - max));
    }
    s
}

/// Read a classic libpcap file of USBPcap records.
fn parse(b: &[u8]) -> anyhow::Result<Vec<Record>> {
    anyhow::ensure!(b.len() >= 24, "file is too short to be a pcap");

    let magic = u32le(b, 0);
    if magic == 0x0a0d_0d0a {
        anyhow::bail!(
            "this is a pcapng file. USBPcapCMD writes classic pcap; if you saved it from \
             Wireshark, use File > Save As and choose \"Wireshark/tcpdump/... - pcap\"."
        );
    }
    anyhow::ensure!(
        magic == 0xa1b2_c3d4 || magic == 0xa1b2_3c4d,
        "not a little-endian pcap file (magic {magic:#010x}); big-endian captures are not \
         produced by USBPcap"
    );
    let linktype = u32le(b, 20);
    anyhow::ensure!(
        linktype == DLT_USBPCAP,
        "link type is {linktype}, not USBPcap ({DLT_USBPCAP}) — is this a network capture?"
    );

    let mut out = Vec::new();
    let mut off = 24;
    let mut frame = 0usize;
    let mut t0: Option<f64> = None;
    while off + 16 <= b.len() {
        let ts = u32le(b, off) as f64 + u32le(b, off + 4) as f64 / 1_000_000.0;
        let t = ts - *t0.get_or_insert(ts);
        let incl = u32le(b, off + 8) as usize;
        off += 16;
        if off + incl > b.len() {
            eprintln!("warning: truncated final record, stopping");
            break;
        }
        let rec = &b[off..off + incl];
        off += incl;
        frame += 1;

        // USBPCAP_BUFFER_PACKET_HEADER, packed:
        //   0 headerLen u16 | 2 irpId u64 | 10 status u32 | 14 function u16
        //  16 info u8 | 17 bus u16 | 19 device u16 | 21 endpoint u8
        //  22 transfer u8 | 23 dataLength u32   (= 27 bytes; control adds stage)
        if rec.len() < 27 {
            continue;
        }
        let header_len = u16le(rec, 0) as usize;
        let info = rec[16];
        let transfer = rec[22];
        let data_len = u32le(rec, 23) as usize;
        if header_len > rec.len() {
            continue;
        }
        let stage = if transfer == 2 && header_len >= 28 {
            Some(rec[27])
        } else {
            None
        };
        let end = (header_len + data_len).min(rec.len());
        out.push(Record {
            frame,
            t,
            bus: u16le(rec, 17),
            device: u16le(rec, 19),
            endpoint: rec[21],
            transfer,
            // USBPCAP_INFO_PDO_TO_FDO: the packet travelled device -> host.
            from_device: info & 0x01 != 0,
            stage,
            data: rec[header_len..end].to_vec(),
        });
    }
    Ok(out)
}
