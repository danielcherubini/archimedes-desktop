import type { VariantConfig } from "../types";
import { setDot, createFieldBuffer } from "../math";

export const chase: VariantConfig = {
  totalFrames: 48,
  interval: 60,
  gridSize: [5, 4],
  compute: (frame, totalFrames, width, height, _ctx) => {
    const field = createFieldBuffer(width);
    const progress = frame / totalFrames;
    const pixelCols = width * 2;
    const drawableHeight = Math.min(height, 4);
    const row = Math.floor((drawableHeight - 1) / 2);
    const head = Math.floor(progress * pixelCols) % pixelCols;

    for (let i = 0; i < 4; i++) {
      const pc = (head - i + pixelCols) % pixelCols;
      const charIdx = Math.floor(pc / 2);
      const dc = pc % 2;
      field[charIdx] = setDot(field[charIdx], row, dc);

      if (i === 0) {
        field[charIdx] = setDot(field[charIdx], Math.max(0, row - 1), dc);
        field[charIdx] = setDot(field[charIdx], Math.min(drawableHeight - 1, row + 1), dc);
      }
    }

    return field;
  },
};
