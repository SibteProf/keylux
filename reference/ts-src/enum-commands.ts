/** Enumerate READ commands (bit7 set) - read-only, no writes. */
import HID from "node-hid";
import { listCandidates, selectRgbInterface } from "./device.js";
import { HEADER_LEN, PACKET_LEN, buildPacket } from "./protocol.js";
const t = selectRgbInterface(listCandidates());
if (!t?.path) throw new Error("no iface");
const dev = new HID.HID(t.path);
const seen = new Map<string, number[]>();
for (let cmd = 0x80; cmd <= 0xff; cmd++) {
  let r: Uint8Array;
  try {
    dev.sendFeatureReport(buildPacket(cmd, [0,0,0,0], 0x0200));
    r = Uint8Array.from(dev.getFeatureReport(0x06, PACKET_LEN));
  } catch { continue; }
  if (r[1] !== cmd) continue;                    // command not echoed => unsupported
  const body = r.slice(HEADER_LEN, HEADER_LEN + 256);
  const allFF = body.every(b => b === 0xff);
  const allZero = body.every(b => b === 0);
  if (allFF || allZero) continue;                // staging buffer or empty
  const key = Buffer.from(body.slice(0, 32)).toString("hex");
  if (!seen.has(key)) seen.set(key, []);
  seen.get(key)!.push(cmd);
}
console.log(`distinct non-trivial regions: ${seen.size}\n`);
for (const [key, cmds] of seen) {
  console.log(`cmds ${cmds.map(c=>"0x"+c.toString(16)).join(", ")}`);
  console.log(`  ${key.match(/../g)!.join(" ")}`);
}
dev.close();
