import { readFileSync, existsSync } from "node:fs";

/**
 * LED index -> physical key, for the AULA F75.
 *
 * The LED order is the keyboard's own key-matrix order, which the firmware
 * exposes via command 0x83 (4 bytes per entry, HID usage code in byte 3).
 * Decoding that table gives the list below.
 *
 * VERIFIED on hardware: writing a single 0xFF byte at buffer offset 1 lit the
 * backtick key red — matching index 1 here, and confirming both this ordering
 * and the planar buffer layout.
 *
 * The order is column-major: down the leftmost physical column (Esc, `, Tab,
 * Caps, LShift, LCtrl), then the next column, and so on. Nulls are matrix
 * positions with no key fitted.
 */
export const LED_ORDER: (string | null)[] = [
  "Esc", "`", "Tab", "Caps", "LShift", "LCtrl", null, "1",        //  0-7
  "Q", "A", "Z", "LWin", "F1", "2", "W", "S",                     //  8-15
  "X", "LAlt", "F2", "3", "E", "D", "C", null,                    // 16-23
  "F3", "4", "R", "F", "V", null, "F4", "5",                      // 24-31
  "T", "G", "B", "Space", "F5", "6", "Y", "H",                    // 32-39
  "N", null, "F6", "7", "U", "J", "M", null,                      // 40-47
  "F7", "8", "I", "K", ",", "Fn", "F8", "9",                      // 48-55
  "O", "L", ".", "RCtrl", "F9", "0", "P", ";",                    // 56-63
  "/", null, "F10", null, "[", "'", "RShift", null,               // 64-71
  "F11", "=", "]", null, null, "Left", "F12", "Backspace",        // 72-79
  "\\", "Enter", "Up", "Down", "Knob", "Del", "PgUp", "PgDn",     // 80-87
  "End", "Right",                                                 // 88-89
];

export type Key = { name: string; row: number; x: number; w: number; led: number };

/**
 * Physical geometry: the row each key sits in and its centre in 1u units from
 * the left edge. The wave samples this, so the animation sweeps across the
 * board in real space rather than in wiring order.
 */
const ROWS: [name: string, width: number][][] = [
  [["Esc", 1], ["F1", 1], ["F2", 1], ["F3", 1], ["F4", 1], ["F5", 1], ["F6", 1],
   ["F7", 1], ["F8", 1], ["F9", 1], ["F10", 1], ["F11", 1], ["F12", 1],
   ["Del", 1], ["Knob", 1]],
  [["`", 1], ["1", 1], ["2", 1], ["3", 1], ["4", 1], ["5", 1], ["6", 1], ["7", 1],
   ["8", 1], ["9", 1], ["0", 1], ["-", 1], ["=", 1], ["Backspace", 2], ["PgUp", 1]],
  [["Tab", 1.5], ["Q", 1], ["W", 1], ["E", 1], ["R", 1], ["T", 1], ["Y", 1], ["U", 1],
   ["I", 1], ["O", 1], ["P", 1], ["[", 1], ["]", 1], ["\\", 1.5], ["PgDn", 1]],
  [["Caps", 1.75], ["A", 1], ["S", 1], ["D", 1], ["F", 1], ["G", 1], ["H", 1], ["J", 1],
   ["K", 1], ["L", 1], [";", 1], ["'", 1], ["Enter", 2.25], ["Home", 1]],
  [["LShift", 2.25], ["Z", 1], ["X", 1], ["C", 1], ["V", 1], ["B", 1], ["N", 1], ["M", 1],
   [",", 1], [".", 1], ["/", 1], ["RShift", 1.75], ["Up", 1], ["End", 1]],
  [["LCtrl", 1.25], ["LWin", 1.25], ["LAlt", 1.25], ["Space", 6.25], ["Fn", 1],
   ["RCtrl", 1], ["Left", 1], ["Down", 1], ["Right", 1]],
];

function buildKeys(): Key[] {
  const ledOf = new Map<string, number>();
  LED_ORDER.forEach((name, i) => {
    if (name && !ledOf.has(name)) ledOf.set(name, i);
  });

  const keys: Key[] = [];
  ROWS.forEach((row, rowIdx) => {
    let x = 0;
    for (const [name, w] of row) {
      const led = ledOf.get(name);
      if (led !== undefined) keys.push({ name, row: rowIdx, x: x + w / 2, w, led });
      x += w;
    }
  });
  return keys;
}

function loadKeys(): Key[] {
  const keys = buildKeys();
  const path = process.env.AULA_KEYMAP ?? "keymap.json";
  if (!existsSync(path)) return keys;
  try {
    const override = JSON.parse(readFileSync(path, "utf8")) as Record<string, number>;
    let applied = 0;
    for (const k of keys) {
      if (Object.prototype.hasOwnProperty.call(override, k.name)) {
        k.led = override[k.name];
        applied++;
      }
    }
    console.error(`keymap.json: applied ${applied} LED index overrides`);
  } catch (e) {
    console.error(`keymap.json ignored (${(e as Error).message})`);
  }
  return keys;
}

export const KEYS: Key[] = loadKeys();
export const MAX_X = Math.max(...KEYS.map((k) => k.x));
export const MAX_ROW = Math.max(...KEYS.map((k) => k.row));
