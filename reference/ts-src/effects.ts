import { LED_SLOTS, STREAM_SLOTS, type Rgb } from "./protocol.js";

/** The colour table holds exactly 128 key slots. */
export const MAX_SLOTS = LED_SLOTS;
import { KEYS, MAX_X, MAX_ROW } from "./keymap.js";

/** h in [0,1), s and v in [0,1]. */
export function hsv(h: number, s: number, v: number): Rgb {
  const i = Math.floor(h * 6);
  const f = h * 6 - i;
  const p = v * (1 - s);
  const q = v * (1 - f * s);
  const t = v * (1 - (1 - f) * s);
  let r: number, g: number, b: number;
  switch (i % 6) {
    case 0: [r, g, b] = [v, t, p]; break;
    case 1: [r, g, b] = [q, v, p]; break;
    case 2: [r, g, b] = [p, v, t]; break;
    case 3: [r, g, b] = [p, q, v]; break;
    case 4: [r, g, b] = [t, p, v]; break;
    default: [r, g, b] = [v, p, q]; break;
  }
  return { r: r * 255, g: g * 255, b: b * 255 };
}

/** A full 128-slot frame with every slot set to one colour. */
export function solidFrame(c: Rgb): Rgb[] {
  return Array.from({ length: MAX_SLOTS }, () => ({ ...c }));
}

export const BLACK_FRAME = (): Rgb[] => solidFrame({ r: 0, g: 0, b: 0 });

export type WaveOptions = {
  /** Wave travel speed in board-widths per second. */
  speed: number;
  /** How many full hue cycles span the board. */
  cycles: number;
  /** Diagonal tilt: how much the row shifts the wave phase. */
  tilt: number;
  brightness: number;
};

/**
 * Render one frame of the travelling rainbow wave at time `t` seconds.
 *
 * The wave is sampled from each key's *physical* position, so it sweeps
 * smoothly across the board rather than jumping around in wiring order.
 * Slots with no key mapped to them stay dark.
 */
export function waveFrame(t: number, opts: WaveOptions): (Rgb | null)[] {
  const frame: (Rgb | null)[] = new Array(MAX_SLOTS).fill(null);
  for (const k of KEYS) {
    if (k.led < 0 || k.led >= MAX_SLOTS) continue;
    const nx = k.x / MAX_X;
    const ny = MAX_ROW > 0 ? k.row / MAX_ROW : 0;
    const phase = nx * opts.cycles + ny * opts.tilt - t * opts.speed;
    const h = ((phase % 1) + 1) % 1;
    frame[k.led] = hsv(h, 1, opts.brightness);
  }
  return frame;
}

/**
 * A bright bar sweeping left to right over a dark background.
 * Deliberately unambiguous: a bar only *looks* like a bar if the slot -> key
 * map is spatially correct, so this doubles as a map check.
 */
export function sweepFrame(t: number, brightness = 1, sweepSeconds = 1.6): (Rgb | null)[] {
  const WIDTH = 0.16;
  const head = ((t / sweepSeconds) % 1) * (1 + 2 * WIDTH) - WIDTH;
  const frame: (Rgb | null)[] = new Array(STREAM_SLOTS).fill(null);
  for (const k of KEYS) {
    if (k.led < 0 || k.led >= STREAM_SLOTS) continue;
    const d = Math.abs(k.x / MAX_X - head);
    const v = d < WIDTH ? Math.pow(1 - d / WIDTH, 2) * brightness : 0;
    frame[k.led] = { r: 255 * v, g: 255 * v, b: 255 * v * 0.9 + 40 * (1 - v) * brightness };
  }
  return frame;
}


/**
 * ---------------------------------------------------------------- text
 *
 * The F75's key grid is about 16 columns x 6 rows, and the rows are
 * physically STAGGERED, so many row/column positions have no key under them.
 * A 3-wide scrolling font is unreadable on that: letters like B and E have a
 * full-height first column, which just reads as a moving bar.
 *
 * So text is shown one BIG letter at a time - 5x5, centred, held briefly -
 * which survives the stagger far better.
 */
/**
 * 5 wide x 4 tall.
 *
 * Four rows, not five: below the F-row the board only has four full-width
 * rows (number, Tab, Caps, Shift). Row 5 is just the spacebar and a few
 * modifiers, so a 5-row glyph loses its bottom bar entirely and E renders
 * as F.
 */
const FONT4: Record<string, string[]> = {
  A: ["01110", "10001", "11111", "10001"],
  B: ["11110", "11110", "10001", "11110"],
  C: ["01111", "10000", "10000", "01111"],
  D: ["11110", "10001", "10001", "11110"],
  E: ["11111", "11100", "10000", "11111"],
  F: ["11111", "11100", "10000", "10000"],
  G: ["01111", "10000", "10011", "01111"],
  H: ["10001", "11111", "10001", "10001"],
  I: ["11111", "00100", "00100", "11111"],
  J: ["00111", "00010", "10010", "01100"],
  K: ["10010", "11100", "10010", "10001"],
  L: ["10000", "10000", "10000", "11111"],
  M: ["10001", "11011", "10101", "10001"],
  N: ["10001", "11001", "10011", "10001"],
  O: ["01110", "10001", "10001", "01110"],
  P: ["11110", "10001", "11110", "10000"],
  Q: ["01110", "10001", "10011", "01111"],
  R: ["11110", "11110", "10010", "10001"],
  S: ["01111", "01110", "00001", "11110"],
  T: ["11111", "00100", "00100", "00100"],
  U: ["10001", "10001", "10001", "01110"],
  V: ["10001", "10001", "01010", "00100"],
  W: ["10001", "10101", "11011", "10001"],
  X: ["10001", "01010", "01010", "10001"],
  Y: ["10001", "01010", "00100", "00100"],
  Z: ["11111", "00110", "01100", "11111"],
  "0": ["01110", "10011", "11001", "01110"],
  "1": ["00100", "01100", "00100", "01110"],
  "2": ["11110", "00110", "01100", "11111"],
  "3": ["11110", "00110", "00001", "11110"],
  "!": ["00100", "00100", "00000", "00100"],
  "?": ["11110", "00011", "00000", "00100"],
  " ": ["00000", "00000", "00000", "00000"],
};

const GLYPH_H = 4;
const GLYPH_W = 5;


export type TextOptions = {
  /** Seconds each letter is held on screen. */
  hold: number;
  brightness: number;
  /** Hue of the lit pixels, 0..1. Cycles per letter when cycleHue is set. */
  hue: number;
  cycleHue: boolean;
  /** Top keyboard row the 5-row glyph occupies. 1 skips the F-row. */
  topRow: number;
  /** Leftmost column of the 5-wide glyph. */
  leftCol: number;
};

/**
 * One frame of the message, shown a letter at a time.
 *
 * Each letter fades in and out slightly so the change between repeated
 * letters is visible rather than looking like a static image.
 */
export function textFrame(t: number, text: string, opts: TextOptions): (Rgb | null)[] {
  const chars = [...text.toUpperCase()];
  const idx = Math.floor(t / opts.hold) % chars.length;
  const phase = (t % opts.hold) / opts.hold;
  const glyph = FONT4[chars[idx]] ?? FONT4["?"];

  // brief dip between letters so repeats are distinguishable
  const env = phase < 0.12 ? phase / 0.12 : phase > 0.88 ? (1 - phase) / 0.12 : 1;
  const hue = opts.cycleHue ? (opts.hue + idx * 0.18) % 1 : opts.hue;
  const on = hsv(hue, 1, opts.brightness * env);

  const frame: (Rgb | null)[] = new Array(STREAM_SLOTS).fill(null);
  for (const k of KEYS) {
    if (k.led < 0 || k.led >= STREAM_SLOTS) continue;
    // Very wide keys (the spacebar) are one LED spanning ~6 columns. Lighting
    // one as a single glyph pixel swamps the letter, so leave them dark.
    if (k.w >= 3) {
      frame[k.led] = { r: 0, g: 0, b: 0 };
      continue;
    }
    const col = Math.round(k.x - 0.5) - opts.leftCol;
    const row = k.row - opts.topRow;
    const lit =
      row >= 0 && row < GLYPH_H && col >= 0 && col < GLYPH_W && glyph[row][col] === "1";
    frame[k.led] = lit ? { ...on } : { r: 0, g: 0, b: 0 };
  }
  return frame;
}

/**
 * The whole message scrolling left -> right across the four full-width rows.
 *
 * Uses the same 5x4 glyphs as the per-letter mode, so letters stay legible;
 * about 2.5 of them are on the board at once. The word starts fully off the
 * left edge and travels right until it is fully off the right edge, then
 * repeats.
 */
export function textScrollFrame(
  t: number,
  text: string,
  opts: TextOptions & { speed: number },
): (Rgb | null)[] {
  // column-major bitmap of the whole message
  const cols: boolean[][] = [];
  for (const ch of text.toUpperCase()) {
    const g = FONT4[ch] ?? FONT4["?"];
    for (let c = 0; c < GLYPH_W; c++) {
      cols.push(Array.from({ length: GLYPH_H }, (_, r) => g[r][c] === "1"));
    }
    cols.push(new Array(GLYPH_H).fill(false)); // 1 column gap between letters
  }

  const boardCols = Math.round(MAX_X) + 1;
  const travel = cols.length + boardCols;
  // leftEdge is where bitmap column 0 sits on the board; it increases with
  // time, which moves the word to the RIGHT.
  const leftEdge = -cols.length + ((t * opts.speed) % travel);

  const frame: (Rgb | null)[] = new Array(STREAM_SLOTS).fill(null);
  for (const k of KEYS) {
    if (k.led < 0 || k.led >= STREAM_SLOTS) continue;
    if (k.w >= 3) {
      frame[k.led] = { r: 0, g: 0, b: 0 };
      continue;
    }
    const col = Math.round(k.x - 0.5);
    const row = k.row - opts.topRow;
    let lit = false;
    let hue = opts.hue;
    if (row >= 0 && row < GLYPH_H) {
      const src = col - Math.floor(leftEdge);
      if (src >= 0 && src < cols.length) {
        lit = cols[src][row];
        // tint by which letter this column belongs to, so the word reads as
        // separate letters rather than one blob
        if (opts.cycleHue) hue = (opts.hue + Math.floor(src / (GLYPH_W + 1)) * 0.18) % 1;
      }
    }
    frame[k.led] = lit ? hsv(hue, 1, opts.brightness) : { r: 0, g: 0, b: 0 };
  }
  return frame;
}

/**
 * ---------------------------------------------------------------- bloom
 *
 * Flowers opening and fading across the board.
 *
 * Each flower grows from a point: petals push outward, hold, then fade. The
 * petal shape comes from modulating the radius by angle, so it reads as a
 * flower rather than an expanding disc. Blooms overlap and are additively
 * blended, and the whole pattern loops seamlessly.
 */
type Flower = {
  x: number;      // centre, in 1u columns
  y: number;      // centre, in rows
  birth: number;  // seconds into the cycle
  hue: number;
  petals: number;
  rot: number;
};

/** Small deterministic RNG so the arrangement is stable between runs. */
function rng(seed: number): () => number {
  let s = seed >>> 0;
  return () => {
    s = (s * 1664525 + 1013904223) >>> 0;
    return s / 0x100000000;
  };
}

/** Soft pastel palette: pinks, violets, coral, gold. */
const BLOOM_HUES = [0.95, 0.88, 0.78, 0.62, 0.45, 0.13, 0.05];

function makeGarden(count: number, cycle: number, seed = 20260908): Flower[] {
  const r = rng(seed);
  const flowers: Flower[] = [];
  for (let i = 0; i < count; i++) {
    flowers.push({
      x: 0.6 + r() * (MAX_X - 1.2),
      y: 0.4 + r() * (MAX_ROW - 0.8),
      // spread births evenly, jittered, so blooms do not pulse in lockstep
      birth: ((i + r() * 0.7) / count) * cycle,
      hue: BLOOM_HUES[Math.floor(r() * BLOOM_HUES.length)],
      petals: 4 + Math.floor(r() * 2), // 4 or 5 petals
      rot: r() * Math.PI * 2,
    });
  }
  return flowers;
}

export type BloomOptions = {
  brightness: number;
  /** Seconds for one full loop of the garden. */
  cycle: number;
  /** Seconds a single flower takes to open and fade. */
  life: number;
  /** How many flowers exist in the loop. */
  count: number;
  /** Peak petal radius, in key widths. */
  size: number;
  /** Background glow, 0-255 on blue. 0 is black and looks best. */
  wash: number;
};

let gardenCache: { key: string; flowers: Flower[] } | null = null;

export function bloomFrame(t: number, opts: BloomOptions): (Rgb | null)[] {
  const key = `${opts.count}:${opts.cycle}`;
  if (!gardenCache || gardenCache.key !== key) {
    gardenCache = { key, flowers: makeGarden(opts.count, opts.cycle) };
  }
  const flowers = gardenCache.flowers;

  const acc = KEYS.map(() => ({ r: 0, g: 0, b: 0 }));

  for (const f of flowers) {
    // age within the loop, so the pattern repeats without a seam
    let age = (t - f.birth) % opts.cycle;
    if (age < 0) age += opts.cycle;
    if (age > opts.life) continue;

    const p = age / opts.life;
    // radius eases out: fast open, gentle settle
    const radius = opts.size * (1 - Math.pow(1 - p, 2.2));
    // fade in quickly, hold, fade out slowly
    const alpha = p < 0.18 ? p / 0.18 : p > 0.55 ? Math.pow(1 - (p - 0.55) / 0.45, 1.5) : 1;
    if (alpha <= 0) continue;

    KEYS.forEach((k, i) => {
      const dx = k.x - f.x;
      const dy = k.row - f.y;
      const d = Math.hypot(dx, dy);
      if (d > radius * 1.05) return;

      const theta = Math.atan2(dy, dx);
      // petal silhouette
      const petalR = radius * (0.66 + 0.34 * Math.cos(f.petals * theta + f.rot));
      if (d > petalR) return;

      const falloff = Math.pow(1 - d / Math.max(petalR, 0.001), 0.85);
      const v = falloff * alpha * opts.brightness;

      // warm pale centre grading out into the petal hue
      const centre = Math.pow(Math.max(0, 1 - d / (radius * 0.42)), 1.6);
      const sat = 0.15 + 0.85 * (1 - centre);
      const hue = f.hue * (1 - centre * 0.55) + 0.12 * (centre * 0.55);
      const c = hsv(hue, sat, v);

      acc[i].r += c.r;
      acc[i].g += c.g;
      acc[i].b += c.b;
    });
  }

  const frame: (Rgb | null)[] = new Array(STREAM_SLOTS).fill(null);
  KEYS.forEach((k, i) => {
    if (k.led < 0 || k.led >= STREAM_SLOTS) return;
    const a = acc[i];
    // No background wash. These LEDs are bright even at very low values, so
    // even a "faint" 10/255 on every key washes the whole board out and the
    // flowers disappear into it. Unlit stays properly black.
    const w = opts.wash;
    frame[k.led] = {
      r: Math.min(255, a.r + w * 0.2),
      g: Math.min(255, a.g + w * 0.6),
      b: Math.min(255, a.b + w),
    };
  });
  return frame;
}
