//! OpenRGB SDK client. Hardware protocols remain in the separately running
//! OpenRGB server; this module only speaks its documented TCP protocol.
//!
//! We negotiate version 3 deliberately: newer servers retain this wire format.
//! Sources and upstream-compatible model targets are in `docs/KEYBOARDS.md`.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use crate::{Error, Frame, KeyPos, Result, RgbDevice};

pub mod catalog;

const VERSION: u32 = 3;
const MAX_PACKET: usize = 4 * 1024 * 1024;
const MAX_CONTROLLERS: u32 = 256;
const MAX_LEDS: usize = 4096;
const TIMEOUT: Duration = Duration::from_secs(2);
/// Conservative default, configurable downward. Not a measured hardware limit.
pub const MAX_FPS: u32 = 20;
pub const DEFAULT_ENDPOINT: &str = "127.0.0.1:6742";

fn error(message: impl Into<String>) -> Error {
    Error::OpenRgb(message.into())
}

fn io_error(e: std::io::Error) -> Error {
    error(format!(
        "SDK connection failed: {e}. Start OpenRGB's SDK server and rescan."
    ))
}

#[derive(Clone, Debug, PartialEq)]
struct Mode {
    name: String,
    flags: u32,
    color_mode: u32,
}

#[derive(Clone, Debug, PartialEq)]
struct Zone {
    count: usize,
    width: usize,
    height: usize,
    map: Vec<u32>,
}

/// The server supplies LED order and geometry, including regional layouts.
#[derive(Clone, Debug)]
pub struct Controller {
    pub index: u32,
    pub name: String,
    pub vendor: String,
    pub serial: String,
    pub location: String,
    pub led_count: usize,
    pub layout: Vec<KeyPos>,
    /// A server without a matrix gets an explicitly labelled LED-grid preview.
    pub has_matrix: bool,
    device_type: u32,
    modes: Vec<Mode>,
    active_mode: u32,
}

impl Controller {
    /// A reversible, collision-free identity; never persist a server list index.
    /// Hex-encoded fields keep names and Windows paths safe to paste into CLI
    /// arguments. Location distinguishes keyboards without serial numbers.
    pub fn key(&self) -> String {
        let mut key = String::from("openrgb:");
        for (i, part) in [&self.name, &self.vendor, &self.serial, &self.location]
            .iter()
            .enumerate()
        {
            if i > 0 {
                key.push(':');
            }
            for byte in part.as_bytes() {
                use std::fmt::Write;
                let _ = write!(key, "{byte:02x}");
            }
        }
        key
    }

    pub fn is_keyboard(&self) -> bool {
        self.device_type == 5
    }

    pub fn supports_direct(&self) -> bool {
        self.modes
            .iter()
            .any(|m| m.name == "Direct" && m.flags & (1 << 5) != 0 && m.color_mode == 1)
    }

    fn same_target(&self, other: &Self) -> bool {
        self.key() == other.key()
            && other.is_keyboard()
            && other.supports_direct()
            && self.led_count == other.led_count
            && self.layout == other.layout
    }
}

/// A bounded, synchronous client, owned by the render worker (never the UI).
pub struct Client {
    socket: TcpStream,
}

impl Client {
    pub fn connect(endpoint: SocketAddr) -> Result<Self> {
        let socket =
            TcpStream::connect_timeout(&endpoint, Duration::from_millis(500)).map_err(io_error)?;
        socket.set_read_timeout(Some(TIMEOUT)).map_err(io_error)?;
        socket.set_write_timeout(Some(TIMEOUT)).map_err(io_error)?;
        socket.set_nodelay(true).map_err(io_error)?;
        let mut client = Self { socket };
        let version = client.request(0, 40, &VERSION.to_le_bytes())?;
        if version.len() != 4 || u32::from_le_bytes(version[..4].try_into().unwrap()) < VERSION {
            return Err(error("SDK protocol 3 or newer is required (OpenRGB 0.7+)."));
        }
        client.send(0, 50, b"keylux\0")?;
        Ok(client)
    }

    /// Enumerate keyboards only; unrelated controllers are never selected and
    /// their device-specific zone sizes do not constrain keyboard discovery.
    pub fn controllers(&mut self) -> Result<Vec<Controller>> {
        let data = self.request(0, 0, &[])?;
        if data.len() != 4 {
            return Err(error("invalid controller count response"));
        }
        let count = u32::from_le_bytes(data[..4].try_into().unwrap());
        if count > MAX_CONTROLLERS {
            return Err(error("server reported too many controllers"));
        }
        let mut keyboards = Vec::new();
        for index in 0..count {
            let data = self.request(index, 1, &VERSION.to_le_bytes())?;
            let mut header = Reader {
                data: &data,
                pos: 0,
            };
            if header.u32()? as usize != data.len() {
                return Err(error("controller size mismatch"));
            }
            if header.u32()? == 5 {
                keyboards.push(parse_controller(index, &data)?);
            }
        }
        Ok(keyboards)
    }

    fn controller(&mut self, index: u32) -> Result<Controller> {
        let data = self.request(index, 1, &VERSION.to_le_bytes())?;
        parse_controller(index, &data)
    }

    fn send(&mut self, index: u32, id: u32, data: &[u8]) -> Result<()> {
        let mut packet = Vec::with_capacity(16 + data.len());
        packet.extend_from_slice(b"ORGB");
        packet.extend_from_slice(&index.to_le_bytes());
        packet.extend_from_slice(&id.to_le_bytes());
        packet.extend_from_slice(&(data.len() as u32).to_le_bytes());
        packet.extend_from_slice(data);
        self.socket.write_all(&packet).map_err(io_error)
    }

    fn request(&mut self, index: u32, id: u32, data: &[u8]) -> Result<Vec<u8>> {
        self.send(index, id, data)?;
        let mut header = [0; 16];
        self.socket.read_exact(&mut header).map_err(io_error)?;
        if &header[..4] != b"ORGB" {
            return Err(error("invalid SDK packet signature"));
        }
        let dev = u32::from_le_bytes(header[4..8].try_into().unwrap());
        let packet_id = u32::from_le_bytes(header[8..12].try_into().unwrap());
        let len = u32::from_le_bytes(header[12..16].try_into().unwrap()) as usize;
        // Indices can change after a server rescan. Stop before another write;
        // rediscovery resolves the saved identity to its new index.
        if packet_id == 100 {
            return Err(error(
                "device list changed; reconnecting before further writes",
            ));
        }
        if dev != index || packet_id != id || len > MAX_PACKET {
            return Err(error("unexpected or oversized SDK response"));
        }
        let mut body = vec![0; len];
        self.socket.read_exact(&mut body).map_err(io_error)?;
        Ok(body)
    }
}

/// One keyboard controlled through OpenRGB. No EEPROM/firmware commands.
pub struct OpenRgbKeyboard {
    client: Client,
    info: Controller,
    endpoint: String,
    fps: u32,
    last_write: Option<Instant>,
    last_frame: Option<Frame>,
    ready: bool,
    failed: bool,
}

impl OpenRgbKeyboard {
    /// Re-enumerate at open time; do not trust an index saved during discovery.
    pub fn open(endpoint: SocketAddr, key: &str) -> Result<Self> {
        let mut client = Client::connect(endpoint)?;
        let mut matches = client.controllers()?.into_iter().filter(|c| c.key() == key);
        let info = matches
            .next()
            .ok_or_else(|| error("selected keyboard is no longer present"))?;
        if matches.next().is_some() {
            return Err(error(
                "two controllers have the same identity; select a distinct device in OpenRGB",
            ));
        }
        if !info.is_keyboard() || !info.supports_direct() || info.led_count == 0 {
            return Err(error(
                "selected controller does not expose per-key Direct mode",
            ));
        }
        Ok(Self {
            client,
            info,
            endpoint: endpoint.to_string(),
            fps: MAX_FPS,
            last_write: None,
            last_frame: None,
            ready: false,
            failed: false,
        })
    }

    pub fn info(&self) -> &Controller {
        &self.info
    }
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    fn verify_target(&mut self) -> Result<()> {
        if self.failed {
            return Err(error("connection invalidated; reconnect before writing"));
        }
        let result = self.client.controller(self.info.index).and_then(|current| {
            if !self.info.same_target(&current) {
                return Err(error(
                    "keyboard identity or LED layout changed; rescan required",
                ));
            }
            if self.ready
                && current
                    .modes
                    .get(current.active_mode as usize)
                    .map(|m| m.name.as_str())
                    != Some("Direct")
            {
                return Err(error(
                    "Direct mode changed; another lighting application may be active",
                ));
            }
            Ok(())
        });
        if result.is_err() {
            self.failed = true;
        }
        result
    }
}

impl RgbDevice for OpenRgbKeyboard {
    fn name(&self) -> &str {
        &self.info.name
    }
    fn led_count(&self) -> usize {
        self.info.led_count
    }
    fn layout(&self) -> &[KeyPos] {
        &self.info.layout
    }
    fn max_fps(&self) -> u32 {
        self.fps
    }
    fn set_max_fps(&mut self, fps: u32) {
        self.fps = fps.clamp(1, MAX_FPS);
    }

    fn ensure_per_key_mode(&mut self) -> Result<bool> {
        self.verify_target()?;
        if self.ready {
            return Ok(false);
        }
        // SetCustomMode selects Direct first. Requiring an advertised Direct
        // mode above prevents its Custom/Static fallback on other controllers.
        if let Err(e) = self.client.send(self.info.index, 1100, &[]) {
            self.failed = true;
            return Err(e);
        }
        self.ready = true;
        self.verify_target()?;
        Ok(true)
    }

    fn set_static(&mut self, _frame: &Frame) -> Result<()> {
        Err(error("permanent storage is not supported by the OpenRGB backend; use live preview or the Solid effect"))
    }

    fn stream(&mut self, frame: &Frame) -> Result<()> {
        if frame.len() != self.led_count() {
            return Err(error(format!(
                "frame has {} LEDs; selected keyboard needs {}",
                frame.len(),
                self.led_count()
            )));
        }
        if !self.ready {
            return Err(error("select Direct mode before streaming"));
        }
        if let Some(last) = self.last_write {
            if let Some(wait) =
                Duration::from_secs_f64(1.0 / f64::from(self.fps)).checked_sub(last.elapsed())
            {
                std::thread::sleep(wait);
            }
        }
        // Read back before every frame, including unchanged frames: notices,
        // disconnects, and index reuse must not silently redirect lighting.
        self.verify_target()?;
        if self.last_frame.as_ref() == Some(frame)
            && self
                .last_write
                .is_some_and(|t| t.elapsed() < Duration::from_secs(2))
        {
            std::thread::sleep(Duration::from_secs_f64(1.0 / f64::from(self.fps)));
            return Ok(());
        }
        let mut data = Vec::with_capacity(6 + frame.len() * 4);
        data.extend_from_slice(&((6 + frame.len() * 4) as u32).to_le_bytes());
        data.extend_from_slice(&(frame.len() as u16).to_le_bytes());
        for c in frame.iter() {
            data.extend_from_slice(&[c.r, c.g, c.b, 0]);
        }
        if let Err(e) = self.client.send(self.info.index, 1050, &data) {
            self.failed = true;
            return Err(e);
        }
        self.last_frame = Some(frame.clone());
        self.last_write = Some(Instant::now());
        Ok(())
    }
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}
impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(count)
            .ok_or_else(|| error("invalid data length"))?;
        let bytes = self
            .data
            .get(self.pos..end)
            .ok_or_else(|| error("truncated controller data"))?;
        self.pos = end;
        Ok(bytes)
    }
    fn u16(&mut self) -> Result<usize> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()) as usize)
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn string(&mut self) -> Result<String> {
        let len = self.u16()?;
        let bytes = self.take(len)?;
        if bytes.last() != Some(&0) {
            return Err(error("unterminated SDK string"));
        }
        std::str::from_utf8(&bytes[..len - 1])
            .map(str::to_string)
            .map_err(|_| error("SDK string is not valid UTF-8"))
    }
}

fn parse_controller(index: u32, data: &[u8]) -> Result<Controller> {
    let mut r = Reader { data, pos: 0 };
    if r.u32()? as usize != data.len() {
        return Err(error("controller size mismatch"));
    }
    let device_type = r.u32()?;
    let name = r.string()?;
    let vendor = r.string()?;
    let _description = r.string()?;
    let _version = r.string()?;
    let serial = r.string()?;
    let location = r.string()?;
    let mode_count = r.u16()?;
    let active_mode = r.u32()?;
    let mut modes = Vec::new();
    for _ in 0..mode_count {
        let name = r.string()?;
        let _value = r.u32()?;
        let flags = r.u32()?;
        r.take(8 * 4)?; // speed/brightness bounds, color bounds, speed, brightness
        let _direction = r.u32()?;
        let color_mode = r.u32()?;
        let colors = r.u16()?;
        r.take(colors * 4)?;
        modes.push(Mode {
            name,
            flags,
            color_mode,
        });
    }
    let zone_count = r.u16()?;
    let mut zones = Vec::new();
    let mut total = 0usize;
    for _ in 0..zone_count {
        let _name = r.string()?;
        let _kind = r.u32()?;
        r.take(8)?; // minimum/maximum LED counts
        let count = r.u32()? as usize;
        total = total
            .checked_add(count)
            .ok_or_else(|| error("invalid LED count"))?;
        // Other SDK controllers can be large; this limit is deliberately ample
        // for keyboards while bounding allocations from a faulty server.
        if total > MAX_LEDS {
            return Err(error("controller exceeds LED limit"));
        }
        let matrix_len = r.u16()?;
        let mut zone = Zone {
            count,
            width: 0,
            height: 0,
            map: Vec::new(),
        };
        if matrix_len != 0 {
            let mut matrix = Reader {
                data: r.take(matrix_len)?,
                pos: 0,
            };
            zone.height = matrix.u32()? as usize;
            zone.width = matrix.u32()? as usize;
            let cells = zone
                .height
                .checked_mul(zone.width)
                .ok_or_else(|| error("invalid matrix dimensions"))?;
            if zone.height == 0
                || zone.width == 0
                || zone.height > 32
                || zone.width > 256
                || cells > MAX_LEDS
                || matrix_len != 8 + cells * 4
            {
                return Err(error("invalid matrix dimensions or length"));
            }
            for _ in 0..cells {
                let led = matrix.u32()?;
                if led != u32::MAX && led as usize >= count {
                    return Err(error("matrix LED index is outside its zone"));
                }
                zone.map.push(led);
            }
        }
        zones.push(zone);
    }
    let led_count = r.u16()?;
    if led_count > MAX_LEDS || total > led_count {
        return Err(error("invalid controller LED count"));
    }
    let mut names = Vec::with_capacity(led_count);
    for _ in 0..led_count {
        names.push(r.string()?);
        r.take(4)?; // vendor LED value, not the SDK frame index
    }
    let colors = r.u16()?;
    if colors != led_count {
        return Err(error("LED/color count mismatch"));
    }
    r.take(colors * 4)?;
    if r.pos != data.len() {
        return Err(error("unexpected trailing protocol data"));
    }
    let (layout, has_matrix) = make_layout(&zones, &names)?;
    Ok(Controller {
        index,
        name,
        vendor,
        serial,
        location,
        led_count,
        layout,
        has_matrix,
        device_type,
        modes,
        active_mode,
    })
}

fn make_layout(zones: &[Zone], names: &[String]) -> Result<(Vec<KeyPos>, bool)> {
    let mut layout = Vec::new();
    let mut seen = vec![false; names.len()];
    let mut offset = 0;
    let mut next_row = 0usize;
    let mut has_matrix = false;
    for zone in zones {
        if !zone.map.is_empty() {
            has_matrix = true;
            if next_row + zone.height > 240 {
                return Err(error("keyboard geometry has too many rows"));
            }
            for row in 0..zone.height {
                for col in 0..zone.width {
                    let local = zone.map[row * zone.width + col];
                    if local == u32::MAX {
                        continue;
                    }
                    let led = offset + local as usize;
                    if seen[led] {
                        continue;
                    }
                    seen[led] = true;
                    // A wide key may occupy several adjacent matrix cells.
                    let width = zone.map[row * zone.width + col..(row + 1) * zone.width]
                        .iter()
                        .take_while(|&&n| n == local)
                        .count() as f32;
                    layout.push(KeyPos {
                        name: names[led].clone(),
                        row: (next_row + row) as u8,
                        x: col as f32 + width / 2.0,
                        w: width,
                        led,
                    });
                }
            }
            next_row += zone.height;
        }
        offset += zone.count;
    }
    // Expose auxiliary LEDs and devices without geometry as a labelled grid.
    // Do not guess a physical keyboard layout from the LED wire order.
    let mut extra = 0;
    for (led, name) in names.iter().enumerate() {
        if seen[led] {
            continue;
        }
        let row = next_row + extra / 24;
        if row > u8::MAX as usize {
            return Err(error("LED grid has too many rows"));
        }
        layout.push(KeyPos {
            name: name.clone(),
            row: row as u8,
            x: (extra % 24) as f32 + 0.5,
            w: 1.0,
            led,
        });
        extra += 1;
    }
    Ok((layout, has_matrix))
}

#[cfg(test)]
mod tests;
