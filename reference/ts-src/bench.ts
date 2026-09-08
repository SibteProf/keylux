/** Measure the real cost of a colour-table write. */
import HID from "node-hid";
import { listCandidates, selectRgbInterface } from "./device.js";
import { ADDR_COLORS, CMD, COLOR_LEN, buildPacket, encodeColorTable } from "./protocol.js";
import { solidFrame } from "./effects.js";

const target = selectRgbInterface(listCandidates());
if (!target?.path) throw new Error("no RGB interface");
const dev = new HID.HID(target.path);

const pkt = buildPacket(
  CMD.WRITE_COLORS,
  ADDR_COLORS,
  COLOR_LEN,
  encodeColorTable(solidFrame({ r: 8, g: 0, b: 16 })),
);

const N = 120;
const times: number[] = [];
for (let i = 0; i < N; i++) {
  const t0 = performance.now();
  dev.sendFeatureReport(pkt);
  times.push(performance.now() - t0);
}
times.sort((a, b) => a - b);
const mean = times.reduce((a, b) => a + b, 0) / times.length;
console.log(`sendFeatureReport x${N} (520 bytes)`);
console.log(`  min    ${times[0].toFixed(2)} ms`);
console.log(`  median ${times[N >> 1].toFixed(2)} ms`);
console.log(`  mean   ${mean.toFixed(2)} ms`);
console.log(`  p95    ${times[Math.floor(N * 0.95)].toFixed(2)} ms`);
console.log(`  max    ${times[N - 1].toFixed(2)} ms`);
console.log(`  => sustained ceiling ~${(1000 / mean).toFixed(1)} FPS`);

// Does packet size drive the cost, or is it fixed per-transfer overhead?
for (const len of [64, 128, 256, 512]) {
  const small = buildPacket(CMD.WRITE_COLORS, ADDR_COLORS, len, new Uint8Array(len));
  const t: number[] = [];
  for (let i = 0; i < 40; i++) {
    const t0 = performance.now();
    try {
      dev.sendFeatureReport(small);
    } catch {
      break;
    }
    t.push(performance.now() - t0);
  }
  if (t.length) {
    t.sort((a, b) => a - b);
    console.log(`  declared len=${len}: median ${t[t.length >> 1].toFixed(2)} ms`);
  }
}

dev.close();
