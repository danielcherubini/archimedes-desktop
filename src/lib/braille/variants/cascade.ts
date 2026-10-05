import type { VariantConfig } from "../types";
import { setDot, createFieldBuffer } from "../math";

export const cascade: VariantConfig = {
  totalFrames: 60,
  interval: 40,
  gridSize: [5, 5],
  compute: (frame, totalFrames, width, height, _ctx) => {
    const progress = frame / totalFrames;
    const field = createFieldBuffer(width);
    const leadingEdge = progress * 2;

    for (let pc = 0; pc < width * 2; pc++) {
      const normalizedX = pc / (width * 2);
      for (let row = 0; row < height; row++) {
        const normalizedY = row / height;
        const diagonalSum = normalizedX + normalizedY;
        const delta = Math.abs(diagonalSum - leadingEdge);

        if (delta < 0.2) {
          const charIdx = Math.floor(pc / 2);
          const dc = pc % 2;
          field[charIdx] = setDot(field[charIdx], row, dc);
        }
      }
    }
    return field;
  },
};
