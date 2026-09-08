import HID from "node-hid";
import { writeFileSync } from "node:fs";
import {
  AulaF75,
  MIN_FRAME_GAP_MS,
  PRODUCT_ID,
  VENDOR_ID,
  listCandidates,
  selectRgbInterface,
} from "./device.js";
import { CONFIG_LEN, STREAM_FPS, hexdump, type ChannelOrder } from "./protocol.js";
import {
  MAX_SLOTS,
  bloomFrame,
  solidFrame,
  sweepFrame,
  textFrame,
  textScrollFrame,
  waveFrame,
} from "./effects.js";
import { KEYS } from "./keymap.js";

type Args = { _: string[]; flags: Record<string, string | boolean> };

function parseArgs(argv: string[]): Args {
  const out: Args = { _: [], flags: {} };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a.startsWith("--")) {
      const [k, inline] = a.slice(2).split("=", 2);
      if (inline !== undefined) out.flags[k] = inline;
      else if (argv[i + 1] && !argv[i + 1].startsWith("--")) out.flags[k] = argv[++i];
      else out.flags[k] = true;
    } else out._.push(a);
  }
  return out;
}

const num = (v: string | boolean | undefined, d: number) =>
  v === undefined || typeof v === "boolean" ? d : Number(v);

const hx = (n: number) => `0x${n.toString(16).padStart(4, "0")}`;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

function openDevice(args: Args): AulaF75 {
  const colorAddr =
    typeof args.flags["color-addr"] === "string"
      ? (args.flags["color-addr"] as string).split(/[,: ]/).map((s) => parseInt(s, 16))
      : undefined;
  return AulaF75.open({
    path: typeof args.flags.path === "string" ? args.flags.path : undefined,
    colorAddr,
    order:
      typeof args.flags.order === "string"
        ? (args.flags.order.toLowerCase() as ChannelOrder)
        : undefined,
    verbose: Boolean(args.flags.verbose),
  });
}

// ---------------------------------------------------------------- info

function cmdInfo(): void {
  const all = HID.devices();
  const mine = listCandidates();
  console.log(`Looking for AULA F75 at ${hx(VENDOR_ID)}:${hx(PRODUCT_ID)}\n`);

  if (mine.length === 0) {
    console.log("NOT FOUND. The F75 exposes this interface only in wired USB-C mode.");
    console.log("Set the side switch to wired, plug in the cable, and re-run.\n");
    console.log("HID devices currently enumerated:");
    for (const d of all) {
      console.log(
        `  ${hx(d.vendorId)}:${hx(d.productId)}  usagePage=${hx(d.usagePage ?? 0)} ` +
          `usage=${hx(d.usage ?? 0)}  ${d.manufacturer ?? ""} ${d.product ?? ""}`.trimEnd(),
      );
    }
    return;
  }

  const target = selectRgbInterface(mine);
  console.log(`Found ${mine.length} collection(s):`);
  for (const d of mine) {
    const mark = d.path === target?.path ? "  <== RGB interface" : "";
    console.log(
      `  usagePage=${hx(d.usagePage ?? 0)} usage=${hx(d.usage ?? 0)} iface=${d.interface}` +
        `  ${d.product ?? ""}${mark}`,
    );
    console.log(`    path: ${d.path}`);
  }
}

// ---------------------------------------------------------------- red

function cmdRed(args: Args): void {
  const dev = openDevice(args);
  try {
    console.log(`Using ${dev.info.product ?? "F75"} (${dev.info.path})`);
    // Custom mode FIRST — host colours are only rendered in that mode, and
    // this is a config write, so it must not land after the frame.
    if (dev.ensureCustomMode()) console.log("Switched the board into custom mode.");
    dev.writeFrame(solidFrame({ r: 255, g: 0, b: 0 }));
    console.log(`Wrote ${MAX_SLOTS} slots = full red (cmd 0x06 @ 00 00 01 00, 384 bytes, planar).`);
  } finally {
    dev.close();
  }
}

// ---------------------------------------------------------------- wave

/**
 * Animated effects, over the STREAMING path (cmd 0x08, interleaved, 126
 * slots). The driver itself runs its music-reactive modes at ~21.5 FPS with
 * 46.5 ms gaps, and that is what this matches.
 *
 * Do NOT animate over the static path (cmd 0x06): it renders one frame fine
 * but blanks the board, indicator included, when written repeatedly.
 */
async function cmdWave(args: Args): Promise<void> {
  const effect = String(args.flags.effect ?? args._[0] ?? "wave");
  const fps = num(args.flags.fps, STREAM_FPS);
  const duration = num(args.flags.duration, Infinity);
  const brightness = Math.min(1, Math.max(0, num(args.flags.brightness, 1)));
  const opts = {
    speed: num(args.flags.speed, 0.35),
    cycles: num(args.flags.cycles, 1.2),
    tilt: num(args.flags.tilt, 0.25),
    brightness,
  };
  // `text` takes its message from the first bare argument after the command.
  const message = String(args._[1] ?? args.flags.message ?? "Sibte");
  const textOpts = {
    hold: num(args.flags.hold, 0.8), // seconds per letter
    brightness,
    hue: num(args.flags.hue, 0.5),
    cycleHue: !args.flags["no-cycle"],
    // Row 1 = the number row. Skipping row 0 keeps letters off the F-row,
    // which is a different keycap profile and reads as a separate strip.
    topRow: num(args.flags.row, 1),
    leftCol: num(args.flags.col, 5), // centre a 5-wide glyph on a ~16 col board
    speed: num(args.flags.speed, 2), // columns per second, scrolling mode
  };
  // Scrolling is the default; --letters shows one big letter at a time.
  const perLetter = Boolean(args.flags.letters);
  const bloomOpts = {
    brightness,
    cycle: num(args.flags.cycle, 14),
    life: num(args.flags.life, 5.5),
    count: num(args.flags.count, 13),
    size: num(args.flags.size, 4.2),
    wash: num(args.flags.wash, 0),
  };

  const dev = openDevice(args);
  console.log(`Using ${dev.info.product ?? "F75"} (${dev.info.path})`);
  const label = effect === "text" ? `${effect} "${message}"` : effect;
  console.log(`Effect ${label} at ${fps} FPS via the streaming path (cmd 0x08).`);
  console.log("Ctrl+C to stop.\n");

  const period = 1000 / fps;
  const startT = performance.now();
  let frames = 0;
  let running = true;

  const stop = () => {
    if (!running) return;
    running = false;
    const secs = (performance.now() - startT) / 1000;
    // Deliberately leave the last frame on the board rather than blanking it.
    dev.close();
    console.log(
      `\nStopped. ${frames} frames in ${secs.toFixed(1)}s = ${(frames / secs).toFixed(1)} FPS.`,
    );
    process.exit(0);
  };
  process.on("SIGINT", stop);

  while (running && (performance.now() - startT) / 1000 < duration) {
    const t = (performance.now() - startT) / 1000;
    try {
      const frame =
        effect === "bloom"
          ? bloomFrame(t, bloomOpts)
          : effect === "text"
          ? perLetter
            ? textFrame(t, message, textOpts)
            : textScrollFrame(t, message, textOpts)
          : effect === "sweep"
            ? sweepFrame(t, brightness)
            : waveFrame(t, opts);
      dev.writeStreamFrame(frame);
    } catch (e) {
      console.error(`\nWrite failed on frame ${frames}: ${(e as Error).message}`);
      break;
    }
    frames++;
    if (frames % (fps * 5) === 0) {
      const secs = (performance.now() - startT) / 1000;
      process.stdout.write(`${frames} frames, ${(frames / secs).toFixed(1)} FPS   `);
    }
    const wait = startT + frames * period - performance.now();
    if (wait > 0) await sleep(wait);
  }
  stop();
}

// ---------------------------------------------------------------- dump

function cmdDump(args: Args): void {
  const dev = openDevice(args);
  try {
    const config = dev.readConfig();
    console.log(`--- config block (${CONFIG_LEN} bytes @ 00 00 01 00) ---`);
    console.log(hexdump(config));
    const colors = dev.readColorTable();
    console.log(`\n--- colour table, first 32 slots ---`);
    console.log(hexdump(colors.slice(0, 128)));
    const out = typeof args.flags.out === "string" ? args.flags.out : undefined;
    if (out) {
      writeFileSync(out, Buffer.concat([Buffer.from(config), Buffer.from(colors)]));
      console.log(`\nSaved ${CONFIG_LEN + 512} bytes to ${out}`);
    }
  } finally {
    dev.close();
  }
}

// ---------------------------------------------------------------- main

async function main(): Promise<void> {
  const args = parseArgs(process.argv.slice(2));
  const cmd = args._[0] ?? "info";
  try {
    switch (cmd) {
      case "info":
        cmdInfo();
        break;
      case "red":
        cmdRed(args);
        break;
      case "wave":
      case "sweep":
      case "text":
      case "bloom":
        await cmdWave(args);
        break;
      case "dump":
        cmdDump(args);
        break;
      case "layout":
        for (const k of KEYS) {
          console.log(`${k.name.padEnd(10)} row=${k.row} x=${k.x.toFixed(2)} led=${k.led}`);
        }
        break;
      default:
        console.error(`Unknown command "${cmd}". Use: info | red | wave | sweep | text | bloom | dump | layout`);
        process.exitCode = 1;
    }
  } catch (e) {
    console.error(`\nError: ${(e as Error).message}`);
    process.exitCode = 1;
  }
}

void main();
