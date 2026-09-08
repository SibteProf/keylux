import HID from "node-hid";
import {
  PACKET_LEN,
  REPORT_ID,
  ADDR_COLORS,
  ADDR_CONFIG,
  CMD,
  COLOR_LEN,
  CONFIG_LEN,
  HEADER_LEN,
  DEFAULT_ORDER,
  STREAM_LEN,
  buildPacket,
  encodeColorTable,
  encodeStreamFrame,
  type ChannelOrder,
  type Rgb,
} from "./protocol.js";

export const VENDOR_ID = 0x258a;   // Sinowealth
export const PRODUCT_ID = 0x010c;  // AULA F75, wired USB-C mode
export const RGB_USAGE_PAGE = 0xff13;

/**
 * Minimum idle time between colour-table writes, in milliseconds.
 *
 * A write blocks the controller for ~31 ms. Issuing them back-to-back with no
 * gap starves the 8051's key-scanning loop and the keyboard stops responding
 * to keypresses until it is unplugged. This gap leaves the firmware time to
 * service its own matrix scan. 35 ms of idle on top of a ~31 ms write is a
 * duty cycle just under 50%, which is measured-safe on this unit.
 */
export const MIN_FRAME_GAP_MS = 35;

/**
 * Config bytes that select the custom lighting mode, captured from the vendor
 * driver at the moment its per-key apply worked. Host colours render only
 * while these are set.
 */
/**
 * How long the firmware takes to finish its asynchronous repaint after a
 * config write. Frames written inside this window get overwritten.
 */
export const CONFIG_RELOAD_SETTLE_MS = 400;

/**
 * The full 128-byte config that was live on the keyboard when the vendor
 * driver's PER-KEY apply worked, captured over USB.
 *
 * Patching just config[9]/[10] is NOT enough -- byte 59 (the first effect
 * entry's speed/colour nibble) also differs between per-key and solid mode,
 * and with only the two mode bytes set the board goes completely dark.
 * Writing this block verbatim is what reliably restores per-key rendering.
 */
export const PERKEY_CONFIG = new Uint8Array([
  0x00, 0x03, 0x03, 0x01, 0x00, 0x00, 0x04, 0x04, 0x07, 0x01, 0x15, 0x20,
  0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x02, 0x01, 0x00, 0xff,
  0x0a, 0x00, 0x01, 0x00, 0x01, 0x00, 0x03, 0x01, 0x00, 0x00, 0x00, 0x00,
  0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
  0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0x09, 0x47,
  0x09, 0x47, 0x09, 0x47, 0x09, 0x47, 0x09, 0x47, 0x09, 0x47, 0x09, 0x47,
  0x09, 0x47, 0x09, 0x47, 0x09, 0x47, 0x09, 0x47, 0x09, 0x47, 0x09, 0x47,
  0x09, 0x47, 0x09, 0x47, 0x09, 0x47, 0x09, 0x37, 0x09, 0x37, 0x09, 0x37,
  0x09, 0x37, 0x07, 0x47, 0x07, 0x47, 0x07, 0x44, 0x07, 0x44, 0x07, 0x44,
  0x07, 0x44, 0x07, 0x44, 0x07, 0x44, 0x07, 0x44, 0x04, 0x09, 0x04, 0x04,
  0x04, 0x04, 0x04, 0x04, 0x04, 0x04, 0x5a, 0xa5,
]);

export const CUSTOM_MODE_OFFSETS = { a: 9, b: 10 } as const;

/**
 * PER-KEY mode. Captured from the vendor driver when it applied individual
 * key colours. This is the mode host per-key data renders in.
 */
export const CUSTOM_MODE_VALUES = { a: 0x01, b: 0x15 } as const;

/**
 * SOLID mode, for reference: the firmware takes ONE colour and paints every
 * key with it, and colours arrive via cmd 0x0a / 512 bytes instead. Writing a
 * per-key frame while in this mode collapses it to a single colour.
 */
export const SOLID_MODE_VALUES = { a: 0x00, b: 0x01 } as const;

/**
 * A valid config block ends with this signature. The firmware returns
 * truncated all-zero buffers when it is busy, and this is how we spot them.
 */
export const CONFIG_MAGIC = [0x5a, 0xa5] as const;

export function isValidConfig(cfg: Uint8Array): boolean {
  return (
    cfg.length === CONFIG_LEN &&
    cfg[CONFIG_LEN - 2] === CONFIG_MAGIC[0] &&
    cfg[CONFIG_LEN - 1] === CONFIG_MAGIC[1]
  );
}

export type OpenOptions = {
  /** Override auto-detection with an explicit HID path. */
  path?: string;
  /** Override the colour-table base address (4 bytes). */
  colorAddr?: number[];
  /** Physical channel order of each slot; defaults to GRB on this hardware. */
  order?: ChannelOrder;
  verbose?: boolean;
};

export function listCandidates(): HID.Device[] {
  return HID.devices().filter(
    (d) => d.vendorId === VENDOR_ID && d.productId === PRODUCT_ID,
  );
}

/**
 * Pick the vendor-defined collection that carries RGB traffic.
 *
 * Windows opens keyboard and mouse top-level collections exclusively for the
 * OS, so those handles are unusable even though they share the VID/PID. That
 * leaves the vendor collections — and on this hardware there are three of
 * them, all reporting usage page 0xFF00, so the usage page alone cannot tell
 * them apart. (Published notes cite 0xFF13; Windows reports 0xFF00 here, and
 * the collection index differs between machines, so neither is reliable.)
 *
 * Instead we probe: only the real RGB collection accepts a 520-byte feature
 * report on report ID 6. The other two reject it outright. This is a
 * read-only test — no write command is sent.
 */
export function selectRgbInterface(devices: HID.Device[]): HID.Device | undefined {
  const vendor = devices.filter((d) => (d.usagePage ?? 0) >= 0xff00 && d.path);

  for (const d of vendor) {
    let handle: HID.HID | undefined;
    try {
      handle = new HID.HID(d.path!);
      handle.getFeatureReport(REPORT_ID, PACKET_LEN);
      return d; // accepted the 520-byte feature report
    } catch {
      /* not this one */
    } finally {
      try {
        handle?.close();
      } catch {
        /* ignore */
      }
    }
  }

  // Nothing responded; fall back to the documented usage page if present.
  return vendor.find((d) => d.usagePage === RGB_USAGE_PAGE);
}

export class AulaF75 {
  private dev: HID.HID;
  private colorAddr: number[];
  private order: ChannelOrder;
  private verbose: boolean;
  readonly info: HID.Device;

  private constructor(dev: HID.HID, info: HID.Device, opts: OpenOptions) {
    this.dev = dev;
    this.info = info;
    this.colorAddr = opts.colorAddr ?? [...ADDR_COLORS];
    this.order = opts.order ?? DEFAULT_ORDER;
    this.verbose = opts.verbose ?? false;
  }

  static open(opts: OpenOptions = {}): AulaF75 {
    if (opts.path) {
      const info =
        HID.devices().find((d) => d.path === opts.path) ??
        ({ path: opts.path } as HID.Device);
      return new AulaF75(new HID.HID(opts.path), info, opts);
    }

    const candidates = listCandidates();
    if (candidates.length === 0) {
      throw new Error(
        `No AULA F75 found at ${hex4(VENDOR_ID)}:${hex4(PRODUCT_ID)}.\n` +
          `The F75 only exposes this interface in WIRED USB-C mode — a 2.4G\n` +
          `dongle or Bluetooth link will not work. Set the side switch to the\n` +
          `wired position, plug in the USB-C cable, then re-run.`,
      );
    }

    const target = selectRgbInterface(candidates);
    if (!target?.path) {
      throw new Error(
        `Found the F75 but no vendor collection (usage page 0x${RGB_USAGE_PAGE.toString(16)}).\n` +
          `Run "npm run info" to list what was enumerated.`,
      );
    }

    return new AulaF75(new HID.HID(target.path), target, opts);
  }

  close(): void {
    try {
      this.dev.close();
    } catch {
      /* already closed */
    }
  }

  /**
   * Change the channel order at runtime.
   */
  setOrder(order: ChannelOrder): void {
    this.order = order;
  }

  /**
   * Put the keyboard into the mode where host per-key colours are rendered.
   *
   * Host colours are only displayed while the board is in its custom lighting
   * mode. That mode is selected by two config bytes, captured from the vendor
   * driver at the moment its own per-key apply worked:
   *
   *   config[9]  = 0x00
   *   config[10] = 0x01
   *
   * This is a read-modify-write of the config block, so it touches
   * non-volatile settings — call it ONCE before streaming, never per frame.
   * Everything else in the config (effect table, brightness, signature) is
   * preserved exactly as read.
   *
   * Returns true if a write was actually needed.
   */
  ensureCustomMode(): boolean {
    const cfg = this.readConfig();
    const alreadyPerKey =
      cfg[CUSTOM_MODE_OFFSETS.a] === CUSTOM_MODE_VALUES.a &&
      cfg[CUSTOM_MODE_OFFSETS.b] === CUSTOM_MODE_VALUES.b &&
      cfg[59] === PERKEY_CONFIG[59];
    if (alreadyPerKey) return false;
    this.writeConfig(PERKEY_CONFIG, true);

    // A config write triggers an ASYNCHRONOUS reload inside the firmware: it
    // re-reads its stored lighting state and repaints the LEDs. That repaint
    // lands tens of milliseconds later and will overwrite any frame written
    // in the meantime — which looks exactly like "the colour write silently
    // failed". Wait for it to finish before returning.
    const until = Date.now() + CONFIG_RELOAD_SETTLE_MS;
    while (Date.now() < until) {
      /* busy wait: this must block, a frame must not be written yet */
    }
    return true;
  }

  private send(pkt: Buffer): void {
    if (this.verbose) {
      console.error(
        `-> cmd=0x${pkt[1].toString(16).padStart(2, "0")} ` +
          `addr=${[...pkt.subarray(2, 6)].map((b) => b.toString(16).padStart(2, "0")).join(" ")} ` +
          `len=0x${(pkt[6] | (pkt[7] << 8)).toString(16).padStart(4, "0")}`,
      );
    }
    this.dev.sendFeatureReport(pkt);
  }

  private receive(): Uint8Array {
    // node-hid wants the report id in byte 0 and the full buffer length.
    const raw = this.dev.getFeatureReport(0x06, PACKET_LEN);
    return Uint8Array.from(raw);
  }

  /**
   * Read the 128-byte config block, rejecting corrupt responses.
   *
   * The firmware sometimes returns a truncated, mostly-zero buffer — reliably
   * reproducible by polling while the knob is pressed and the controller is
   * busy reloading its effect table. A valid block always ends with the
   * 0x5A 0xA5 signature, so that is the integrity check.
   *
   * This matters because commit() is read-modify-WRITE: accepting a glitched
   * read would write zeros over the whole config block, wiping the effect
   * table and the signature with it.
   */
  readConfig(attempts = 4): Uint8Array {
    let last: Uint8Array | undefined;
    for (let i = 0; i < attempts; i++) {
      this.send(buildPacket(CMD.READ_CONFIG, ADDR_CONFIG, CONFIG_LEN));
      const cfg = this.receive().slice(HEADER_LEN, HEADER_LEN + CONFIG_LEN);
      last = cfg;
      if (isValidConfig(cfg)) return cfg;
      if (this.verbose) console.error(`readConfig: bad response, retry ${i + 1}/${attempts}`);
    }
    throw new Error(
      `Config read failed ${attempts}x: response missing the 0x5A 0xA5 signature ` +
        `(got ${last ? `${last[126].toString(16)} ${last[127].toString(16)}` : "nothing"}). ` +
        `Refusing to use it — writing this back would corrupt the config block. ` +
        `Let the keyboard settle (do not press the knob) and retry.`,
    );
  }

  /**
   * Write the 128-byte config block.
   *
   * DANGEROUS FOR LIGHTING. A config write makes the firmware reload its
   * stored lighting state, which drops the keyboard OUT of the custom effect.
   * Host per-key colours render only while custom is active, so this call
   * silently kills all colour output until the vendor driver is used to
   * re-select custom.
   *
   * Observed directly: per-key colours were rendering (Esc lit red), one
   * config write was sent, and rendering stopped and did not come back —
   * through mode cycling, power-of-two slot counts, brightness changes and a
   * replug. Nothing in the colour path may call this.
   */
  writeConfig(config: Uint8Array, allowLightingBreak = false): void {
    if (!allowLightingBreak) {
      throw new Error(
        "writeConfig() refused: a config write drops the keyboard out of custom mode " +
          "and stops per-key colours rendering. Pass allowLightingBreak=true only if " +
          "you specifically intend that.",
      );
    }
    if (config.length !== CONFIG_LEN) {
      throw new Error(`config must be ${CONFIG_LEN} bytes, got ${config.length}`);
    }
    // Last line of defence: never write a block that fails the signature
    // check, whatever its provenance. This is the write that could brick the
    // lighting config, so it refuses rather than trusting its caller.
    if (!isValidConfig(config)) {
      throw new Error(
        `Refusing to write a config block missing the 0x5A 0xA5 signature — ` +
          `this would corrupt the keyboard's stored settings.`,
      );
    }
    this.send(buildPacket(CMD.WRITE_CONFIG, ADDR_CONFIG, CONFIG_LEN, config));
  }

  /** Read the raw 512-byte colour table. */
  readColorTable(): Uint8Array {
    this.send(buildPacket(CMD.READ_COLORS, this.colorAddr, COLOR_LEN));
    const resp = this.receive();
    return resp.slice(HEADER_LEN, HEADER_LEN + COLOR_LEN);
  }

  /**
   * Push one full frame: all 128 slots in a single 520-byte feature report.
   *
   * This does NOT latch on its own — see commit(). For animation you latch
   * once up front and then call this per frame.
   */
  writeFrame(colors: (Rgb | null)[]): void {
    const payload = encodeColorTable(colors, this.order);
    this.send(buildPacket(CMD.WRITE_COLORS, this.colorAddr, COLOR_LEN, payload));
  }

  /**
   * Push one ANIMATION frame over the streaming path (cmd 0x08).
   *
   * Use this for anything that updates repeatedly. The static path (0x06)
   * renders a single frame fine but blanks the board when hammered.
   */
  writeStreamFrame(colors: (Rgb | null)[]): void {
    const payload = encodeStreamFrame(colors, this.order);
    this.send(buildPacket(CMD.STREAM_COLORS, this.colorAddr, STREAM_LEN, payload));
  }

  /**
   * Write the colour table as raw bytes, bypassing stride and channel-order
   * encoding entirely.
   *
   * Filling all 512 bytes with 0xFF drives every LED channel to full no
   * matter what the real stride or channel order turns out to be, so it is
   * the one test that cannot be defeated by a wrong format guess: if the
   * board is rendering host data at all, it goes bright white.
   */
  writeRawTable(fill: number | Uint8Array): void {
    const payload =
      typeof fill === "number"
        ? new Uint8Array(COLOR_LEN).fill(fill & 0xff)
        : fill;
    if (payload.length !== COLOR_LEN) {
      throw new Error(`raw table must be ${COLOR_LEN} bytes, got ${payload.length}`);
    }
    this.send(buildPacket(CMD.WRITE_COLORS, this.colorAddr, COLOR_LEN, payload));
  }

  /**
   * Latch the colour table so the LEDs actually show it.
   *
   * IMPORTANT — this is the one part of the protocol that is not publicly
   * documented. Every published source stops here: writing the table stores
   * the colours but the keyboard keeps rendering its built-in effect until
   * the vendor app sends something extra.
   *
   * The implementation below is the best-supported inference rather than a
   * confirmed capture: on Sinowealth boards the per-key table is only shown
   * while the active effect is the "custom/user" effect, and a config write
   * is what selects it. So we read the config, optionally force one byte to
   * select custom mode, and write it back.
   *
   * If your unit does not light up, use `npm run probe -- commit` to sweep
   * candidate mode offsets and values, then pass the pair that works via
   * --mode-offset / --mode-value.
   */
  commit(opts: { modeOffset?: number; modeValue?: number } = {}): void {
    const config = this.readConfig();
    if (opts.modeOffset !== undefined && opts.modeValue !== undefined) {
      if (opts.modeOffset < 0 || opts.modeOffset >= CONFIG_LEN) {
        throw new Error(`mode offset must be 0..${CONFIG_LEN - 1}`);
      }
      config[opts.modeOffset] = opts.modeValue & 0xff;
    }
    this.writeConfig(config, true);
  }
}

export const hex4 = (n: number) => `0x${n.toString(16).padStart(4, "0")}`;
