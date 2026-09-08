/**
 * Decode a Wireshark/USBPcap capture of the AULA vendor driver.
 *
 * This is the ground-truth path. Everything else in this project inferred the
 * protocol from published notes and probing; this reads what the official
 * software genuinely sends, which is the only way left to find how per-key
 * colours are actually latched.
 *
 * Usage:
 *   npx tsx src/parse-capture.ts capture.json
 *   npx tsx src/parse-capture.ts capture.json --all      (don't collapse repeats)
 *
 * Produce capture.json from Wireshark:
 *   File > Export Packet Dissections > As JSON  (with the USB packets shown)
 *
 * What to look for in the output: the command bytes the driver uses that we
 * never tried, and — critically — whatever short packet it sends immediately
 * AFTER writing colour data. That trailing packet is the latch.
 */
import { readFileSync } from "node:fs";

const file = process.argv[2];
const showAll = process.argv.includes("--all");
if (!file) {
  console.error("usage: tsx src/parse-capture.ts <wireshark-export.json> [--all]");
  process.exit(1);
}

type Packet = { hex: string; frame?: string };

/** Pull every hex payload out of the export, whatever nesting Wireshark used. */
function extractPayloads(node: unknown, out: Packet[], frame?: string): void {
  if (node === null || node === undefined) return;
  if (Array.isArray(node)) {
    for (const v of node) extractPayloads(v, out, frame);
    return;
  }
  if (typeof node !== "object") return;

  const obj = node as Record<string, unknown>;
  const frameNo =
    typeof obj["frame.number"] === "string" ? (obj["frame.number"] as string) : frame;

  for (const [key, value] of Object.entries(obj)) {
    // Wireshark renders raw bytes as colon- or space-separated hex strings.
    if (
      typeof value === "string" &&
      /^(?:[0-9a-f]{2}[:\s]){7,}[0-9a-f]{2}$/i.test(value) &&
      (key.includes("capdata") || key.includes("data") || key.includes("payload"))
    ) {
      out.push({ hex: value.replace(/[^0-9a-f]/gi, "").toLowerCase(), frame: frameNo });
    } else {
      extractPayloads(value, out, frameNo);
    }
  }
}

const json = JSON.parse(readFileSync(file, "utf8"));
const payloads: Packet[] = [];
extractPayloads(json, payloads);

// Keep the ones that look like this device's 520-byte report-6 packets.
const reports = payloads.filter((p) => p.hex.length >= 16 && p.hex.startsWith("06"));

console.log(`payloads found: ${payloads.length}`);
console.log(`report-id-6 packets: ${reports.length}\n`);

if (reports.length === 0) {
  console.log("Nothing matched. Check that the export includes USB control");
  console.log("transfers (SET_REPORT/GET_REPORT), not just interrupt traffic.");
  process.exit(0);
}

const b = (hex: string, i: number) => parseInt(hex.slice(i * 2, i * 2 + 2), 16);
const KNOWN: Record<number, string> = {
  0x04: "WRITE_CONFIG (known)",
  0x84: "READ_CONFIG (known)",
  0x0a: "WRITE_COLORS (known)",
  0x8a: "READ_COLORS (known)",
};

type Row = { cmd: number; addr: string; len: number; body: string; frame?: string };
const rows: Row[] = reports.map((p) => ({
  cmd: b(p.hex, 1),
  addr: [2, 3, 4, 5].map((i) => b(p.hex, i).toString(16).padStart(2, "0")).join(" "),
  len: b(p.hex, 6) | (b(p.hex, 7) << 8),
  body: p.hex.slice(16),
  frame: p.frame,
}));

console.log("--- command inventory ---");
const byCmd = new Map<number, number>();
for (const r of rows) byCmd.set(r.cmd, (byCmd.get(r.cmd) ?? 0) + 1);
for (const [cmd, n] of [...byCmd.entries()].sort((a, b) => a[0] - b[0])) {
  const tag = KNOWN[cmd] ?? (cmd & 0x80 ? "read, UNKNOWN" : "write, *** UNKNOWN ***");
  console.log(`  0x${cmd.toString(16).padStart(2, "0")}  x${String(n).padStart(4)}  ${tag}`);
}

console.log("\n--- packet sequence ---");
let last = "";
let repeat = 0;
for (const r of rows) {
  const nonZero = (r.body.match(/[1-9a-f]/g) ?? []).length;
  const sig = `${r.cmd}|${r.addr}|${r.len}`;
  if (!showAll && sig === last) {
    repeat++;
    continue;
  }
  if (repeat > 0) {
    console.log(`      ... x${repeat} more identical`);
    repeat = 0;
  }
  last = sig;
  const flag = KNOWN[r.cmd] ? "" : "   <== UNKNOWN COMMAND";
  console.log(
    `  ${r.frame ? `#${r.frame.padStart(5)}` : "     "}  ` +
      `cmd=0x${r.cmd.toString(16).padStart(2, "0")}  addr=${r.addr}  ` +
      `len=0x${r.len.toString(16).padStart(4, "0")}  payload-nibbles-set=${nonZero}${flag}`,
  );
}
if (repeat > 0) console.log(`      ... x${repeat} more identical`);

// The latch is whatever the driver sends right after colour data.
console.log("\n--- packets following a colour write (latch candidates) ---");
let found = 0;
for (let i = 0; i < rows.length - 1; i++) {
  if (rows[i].cmd === 0x0a || (rows[i].len === 0x0200 && !(rows[i].cmd & 0x80))) {
    const n = rows[i + 1];
    console.log(
      `  after colour write -> cmd=0x${n.cmd.toString(16).padStart(2, "0")} ` +
        `addr=${n.addr} len=0x${n.len.toString(16).padStart(4, "0")}` +
        `  body[0..15]=${n.body.slice(0, 32).match(/../g)?.join(" ") ?? ""}`,
    );
    if (++found >= 10) break;
  }
}
if (found === 0) console.log("  none seen — check the capture covers an 'apply' click");
