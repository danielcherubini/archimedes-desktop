/**
 * Shared procedural-animation math helpers for the braille spinner engine.
 *
 * Used by both the frame generator (`src/lib/braille-loader.ts`) and the
 * per-variant compute functions (`src/lib/braille/variants/*.ts`).
 */

export const DOT_BITS = [
  [0x01, 0x08],
  [0x02, 0x10],
  [0x04, 0x20],
  [0x40, 0x80],
];

export const BRAILLE_BASE = 0x2800;

export function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

export function seededRandom(seed: number): () => number {
  let s = seed;
  return () => {
    s = (s * 1664525 + 1013904223) & 0xffffffff;
    return (s >>> 0) / 0xffffffff;
  };
}

export function smoothstep(t: number): number {
  return t * t * (3 - 2 * t);
}

export function setDot(brailleChar: number, row: number, col: number): number {
  if (row < 0 || row > 3) return brailleChar;
  return brailleChar | DOT_BITS[row][col];
}

export function createFieldBuffer(width: number): number[] {
  return Array.from({ length: width }, () => 0);
}

export function fieldToString(field: number[]): string {
  return field.map((mask) => String.fromCharCode(BRAILLE_BASE + mask)).join("");
}
