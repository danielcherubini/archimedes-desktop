import type { VariantConfig } from "../types";
import { setDot, createFieldBuffer } from "../math";

export const marquee: VariantConfig = {
  totalFrames: 48,
  interval: 55,
  gridSize: [5, 4],
  compute: (frame, totalFrames, width, height, _ctx) => {
    const field = createFieldBuffer(width);
    const pixelCols = width * 2;
    const drawableHeight = Math.min(height, 4);
    const offset = Math.floor((frame / totalFrames) * 8);

    for (let pc = 0; pc < pixelCols; pc++) {
      for (let row = 0; row < drawableHeight; row++) {
        const stripe = (pc + row + offset) % 4;
        if (stripe < 2) {
          const charIdx = Math.floor(pc / 2);
          field[charIdx] = setDot(field[charIdx], row, pc % 2);
        }
      }
    }

    return field;
  },
};
