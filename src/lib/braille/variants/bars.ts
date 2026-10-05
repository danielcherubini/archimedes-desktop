import type { VariantConfig } from "../types";
import { clamp, setDot, createFieldBuffer } from "../math";

export const bars: VariantConfig = {
  totalFrames: 64,
  interval: 50,
  gridSize: [5, 4],
  compute: (frame, totalFrames, width, height, _ctx) => {
    const field = createFieldBuffer(width);
    const progress = frame / totalFrames;
    const pixelCols = width * 2;
    const drawableHeight = Math.min(height, 4);
    const phase = progress * Math.PI * 2;

    for (let pc = 0; pc < pixelCols; pc++) {
      const distanceFromCenter = Math.abs(pc - (pixelCols - 1) / 2) / Math.max(1, pixelCols / 2);
      const wave = (Math.sin(phase - distanceFromCenter * Math.PI * 1.7) + 1) / 2;
      const barHeight = clamp(Math.round(1 + wave * (drawableHeight - 1)), 1, drawableHeight);
      const charIdx = Math.floor(pc / 2);
      const dc = pc % 2;

      for (let i = 0; i < barHeight; i++) {
        const row = drawableHeight - 1 - i;
        field[charIdx] = setDot(field[charIdx], row, dc);
      }
    }

    return field;
  },
};
