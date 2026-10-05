import type { VariantConfig } from "../types";
import { setDot, createFieldBuffer } from "../math";

export const pendulum: VariantConfig = {
  totalFrames: 120,
  interval: 12,
  gridSize: [5, 4],

  compute: (frame, totalFrames, width, height, _ctx) => {
    const field = createFieldBuffer(width);

    const progress = frame / totalFrames;
    const pixelCols = width * 2;

    // ✅ fast natural swing
    const basePhase = progress * Math.PI * 8;

    // ✅ dynamic spatial sine (CRITICAL)
    const spread = Math.sin(progress * Math.PI) * 1.1;

    const threshold = 0.7;

    for (let pc = 0; pc < pixelCols; pc++) {
      // ⭐ sine exists INSIDE braille cell
      const swing = Math.sin(basePhase + pc * spread);

      const center = ((1 - swing) * (height - 1)) / 2;

      for (let row = 0; row < height; row++) {
        if (Math.abs(row - center) < threshold) {
          const charIdx = Math.floor(pc / 2);
          field[charIdx] = setDot(field[charIdx], row, pc % 2);
        }
      }
    }

    return field;
  },
};
