/**
 * AULA F75 (Sinowealth SH68F90-class 8051) HID protocol.
 *
 * Established from public reverse-engineering work (see README "Sources"):
 *   - Wired USB-C mode enumerates as 258a:010c.
 *   - All RGB traffic goes over the vendor-defined HID collection,
 *     usage page 0xFF13, as HID *feature* reports (not output reports).
 *   - A packet is 520 bytes: an 8-byte header followed by 512 bytes of payload.
 *
 *       byte 0      report id, always 0x06
 *       byte 1      command; bit 7 set = read, clear = write
 *       bytes 2..5  target address, big-endian order as observed on the wire
 *       bytes 6..7  payload length, little-endian
 *       bytes 8..   payload, zero padded to 512
 *
 * The per-key colour table is 512 bytes = 170 slots x 3 bytes (RR GG BB), and
 * fits in one packet. VERIFIED against a USBPcap capture of the vendor driver.
 */

export const REPORT_ID = 0x06;
export const HEADER_LEN = 8;
export const PAYLOAD_LEN = 512;
export const PACKET_LEN = HEADER_LEN + PAYLOAD_LEN; // 520

/** Slots in the colour table (512 / 3). Not all map to physical keys. */
export const LED_SLOTS = 128;
/**
 * Bytes per slot. PROVEN: decoding the driver payload at stride 3 yields pure
 * primaries (255,0,0) (0,255,0) (0,0,255) (255,255,0) ... ; stride 4 is noise.
 */
export const STRIDE = 3;

/**
 * LIVE STREAMING format, captured from the driver's music-reactive modes.
 * Completely separate from the static per-key apply:
 *   static  : cmd 0x06, 384 bytes, PLANAR,      128 slots, persisted
 *   stream  : cmd 0x08, 378 bytes, INTERLEAVED, 126 slots, per-frame
 * Hammering the static path with repeated writes blanks the board; the stream
 * path is built for it and the driver runs it at ~21.5 FPS (46.5 ms gaps).
 */
export const STREAM_LEN = 0x017a;            // 378
export const STREAM_SLOTS = STREAM_LEN / 3;  // 126
export const STREAM_FPS = 21;

export const CMD = {
  WRITE_CONFIG: 0x04,
  READ_CONFIG: 0x84,
  /**
   * Per-key colour write. VERIFIED: the board lights up from this.
   *
   * The ADDRESS is the part that matters and the part every earlier attempt
   * got wrong. 0x0a at 00 00 00 00 is a staging buffer: it accepts writes and
   * reads them back byte-perfect, but is not wired to the LEDs. 0x0a at
   * 00 00 01 00 is the real framebuffer.
   *
   * (The driver also has a 0x06 / 0x0180 path used for some UI modes. 0x0a is
   * the one that drives a full frame.)
   */
  /** Static per-key apply: planar, 384 bytes, persists. */
  WRITE_COLORS: 0x06,
  /** Live streaming: interleaved, 378 bytes, for animation. */
  STREAM_COLORS: 0x08,
  READ_COLORS: 0x86,
} as const;

/** Address of the 128-byte main config block. */
export const ADDR_CONFIG = [0x00, 0x00, 0x01, 0x00] as const;
export const CONFIG_LEN = 0x0080;

/**
 * Address of the per-key colour table. Same address as the config block --
 * the COMMAND selects the region, the address selects real-vs-staging.
 * 00 00 00 00 here silently does nothing visible.
 */
export const ADDR_COLORS = [0x00, 0x00, 0x01, 0x00] as const;

/** 170 slots x 3 bytes = 512. Captured length is exactly 0x0200. */
export const COLOR_LEN = 0x0180;

export type Rgb = { r: number; g: number; b: number };

const clamp255 = (n: number) => (n < 0 ? 0 : n > 255 ? 255 : Math.round(n));

/** Build a 520-byte feature-report buffer. */
export function buildPacket(
  cmd: number,
  addr: readonly number[],
  length: number,
  payload?: Uint8Array,
): Buffer {
  if (addr.length !== 4) throw new Error("address must be 4 bytes");
  const pkt = Buffer.alloc(PACKET_LEN, 0);
  pkt[0] = REPORT_ID;
  pkt[1] = cmd & 0xff;
  pkt[2] = addr[0];
  pkt[3] = addr[1];
  pkt[4] = addr[2];
  pkt[5] = addr[3];
  pkt[6] = length & 0xff;        // little-endian length
  pkt[7] = (length >> 8) & 0xff;
  if (payload) {
    if (payload.length > PAYLOAD_LEN) {
      throw new Error(`payload ${payload.length} exceeds ${PAYLOAD_LEN} bytes`);
    }
    Buffer.from(payload).copy(pkt, HEADER_LEN);
  }
  return pkt;
}

/**
 * Physical channel order within each 3-byte slot.
 *
 * The capture shows the driver writing ff 00 00 into slot 0 for a key the
 * user had set to pure RED, so byte 0 is the red channel and the default is
 * plain "rgb". (The capture could not disambiguate green from blue, because
 * the two other coloured keys both had only their third byte set — so if
 * green and blue come out swapped, use --order rbg.)
 */
export type ChannelOrder = "rgb" | "grb" | "brg" | "rbg" | "gbr" | "bgr";

export const DEFAULT_ORDER: ChannelOrder = "rgb";

/**
 * Encode colours into the 512-byte table: 170 slots x 3 bytes, no padding.
 * Global brightness lives in the config block.
 */
export function encodeColorTable(
  colors: (Rgb | null)[],
  order: ChannelOrder = DEFAULT_ORDER,
): Uint8Array {
  const buf = new Uint8Array(COLOR_LEN);
  for (let i = 0; i < LED_SLOTS; i++) {
    const c = colors[i];
    if (!c) continue;
    const v = { r: clamp255(c.r), g: clamp255(c.g), b: clamp255(c.b) };
    // PLANAR: all reds, then all greens, then all blues.
    buf[i] = v[order[0] as "r" | "g" | "b"];
    buf[LED_SLOTS + i] = v[order[1] as "r" | "g" | "b"];
    buf[2 * LED_SLOTS + i] = v[order[2] as "r" | "g" | "b"];
  }
  return buf;
}

/** Encode a frame for the STREAMING path: interleaved RGB, 126 slots. */
export function encodeStreamFrame(
  colors: (Rgb | null)[],
  order: ChannelOrder = DEFAULT_ORDER,
): Uint8Array {
  const buf = new Uint8Array(STREAM_LEN);
  for (let i = 0; i < STREAM_SLOTS; i++) {
    const c = colors[i];
    if (!c) continue;
    const o = i * 3;
    const v = { r: clamp255(c.r), g: clamp255(c.g), b: clamp255(c.b) };
    buf[o] = v[order[0] as "r" | "g" | "b"];
    buf[o + 1] = v[order[1] as "r" | "g" | "b"];
    buf[o + 2] = v[order[2] as "r" | "g" | "b"];
  }
  return buf;
}

export function decodeColorTable(buf: Uint8Array): Rgb[] {
  const out: Rgb[] = [];
  for (let i = 0; i < LED_SLOTS; i++)
    out.push({ r: buf[i] ?? 0, g: buf[LED_SLOTS + i] ?? 0, b: buf[2 * LED_SLOTS + i] ?? 0 });
  return out;
}

export function hexdump(buf: Uint8Array, width = 16): string {
  const lines: string[] = [];
  for (let i = 0; i < buf.length; i += width) {
    const slice = Array.from(buf.slice(i, i + width));
    const hex = slice.map((b) => b.toString(16).padStart(2, "0")).join(" ");
    const asc = slice.map((b) => (b >= 32 && b < 127 ? String.fromCharCode(b) : ".")).join("");
    lines.push(`${i.toString(16).padStart(4, "0")}  ${hex.padEnd(width * 3 - 1)}  ${asc}`);
  }
  return lines.join("\n");
}
