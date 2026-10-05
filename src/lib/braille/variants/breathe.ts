import type { VariantConfig } from "../types";
import { setDot, createFieldBuffer, seededRandom } from "../math";

export const breathe: VariantConfig = {
  totalFrames: 40,
  interval: 40,
  gridSize: [1, 6],

  compute: (frame, totalFrames, width, height, _ctx) => {
    const field = createFieldBuffer(width);
    const progress = frame / totalFrames;

    const alpha = 0.5 + 0.5 * Math.sin(progress * Math.PI * 2);

    // -----------------------------
    // loop index (changes ONLY after animation ends)
    // -----------------------------
    const loopIndex = Math.floor(frame / totalFrames);

    // -----------------------------
    // checker dots
    // -----------------------------
    const dots: { pc: number; row: number }[] = [];

    for (let pass = 0; pass < 2; pass++) {
      for (let pc = 0; pc < width * 2; pc++) {
        for (let row = 0; row < height; row++) {
          if ((pc + row) % 2 === pass) {
            dots.push({ pc, row });
          }
        }
      }
    }

    // -----------------------------
    // ✅ hole chosen once per loop
    // -----------------------------
    const rand = seededRandom(9001 + loopIndex);
    const holeIndex = Math.floor(rand() * dots.length);

    const maxDots = dots.length - 1;
    const activeDots = Math.floor(alpha * maxDots);

    let placed = 0;

    for (let i = 0; i < dots.length; i++) {
      if (i === holeIndex) continue;
      if (placed >= activeDots) break;

      const { pc, row } = dots[i];

      const charIdx = Math.floor(pc / 2);
      const dc = pc % 2;

      field[charIdx] = setDot(field[charIdx], row, dc);

      placed++;
    }

    return field;
  },
};
