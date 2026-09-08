/** Restore the colour table captured in baseline.bin. */
import HID from "node-hid";
import { readFileSync } from "node:fs";
import { listCandidates, selectRgbInterface } from "./device.js";
import { ADDR_COLORS, CMD, COLOR_LEN, buildPacket } from "./protocol.js";

const t = selectRgbInterface(listCandidates());
if (!t?.path) throw new Error("no RGB interface");
const dev = new HID.HID(t.path);
const base = new Uint8Array(readFileSync("baseline.bin"));
dev.sendFeatureReport(
  buildPacket(CMD.WRITE_COLORS, ADDR_COLORS, COLOR_LEN, base.slice(128, 128 + COLOR_LEN)),
);
console.log("colour table restored from baseline.bin");
dev.close();
