import type { VariantConfig } from "../types";
import { setDot, createFieldBuffer } from "../math";

export const sort: VariantConfig = {
  totalFrames: 100,
  interval: 40,
  gridSize: [5, 6],

  compute: (frame, totalFrames, width, height, ctx) => {
    const progress = frame / totalFrames;
    const pixelCols = width * 2;

    const field = createFieldBuffer(width);

    // sorting front
    const cursor = progress * pixelCols;

    for (let pc = 0; pc < pixelCols; pc++) {
      const charIdx = Math.floor(pc / 2);
      const dc = pc % 2;

      let fillHeight;

      // =========================
      // ✅ SORTED REGION
      // =========================
      if (pc < cursor - 1) {
        fillHeight = ctx.target[pc];
      }

      // =========================
      // ✅ ACTIVE FRONT
      // =========================
      else if (Math.abs(pc - cursor) < 2) {
        const blend = 1 - Math.abs(pc - cursor) / 2;

        const ease = blend * blend * (3 - 2 * blend);

        fillHeight = ctx.shuffled[pc] + (ctx.target[pc] - ctx.shuffled[pc]) * ease;

        // flash = comparison
        if (blend > 0.7) {
          for (let r = 0; r < height; r++) {
            field[charIdx] = setDot(field[charIdx], r, dc);
          }
          continue;
        }
      }

      // =========================
      // ✅ UNSORTED REGION
      // =========================
      else {
        fillHeight = ctx.shuffled[pc] + Math.sin(progress * Math.PI * 12 + pc * 2.3) * 0.8;
      }

      fillHeight = Math.max(0, Math.min(height - 1, fillHeight));

      // ✅ IMPORTANT FIX:
      // cumulative stacking (NO ERASURE)
      for (let r = Math.floor(fillHeight); r < height; r++) {
        field[charIdx] = setDot(field[charIdx], r, dc);
      }
    }

    return field;
  },
};
