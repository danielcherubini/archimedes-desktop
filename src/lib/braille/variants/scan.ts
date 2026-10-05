import type { VariantConfig } from "../types";
import { setDot, createFieldBuffer } from "../math";

export const scan: VariantConfig = {
  totalFrames: 60,
  interval: 40,
  gridSize: [4, 4],
  compute: (frame, _totalFrames, width, height, _ctx) => {
    const period = 900;
    const t = frame * 40;
    const progress = (t / period) % 1;
    const field = createFieldBuffer(width);
    const scanX = progress * (width * 2 - 1);
    const sigma = 0.45;

    for (let pc = 0; pc < width * 2; pc++) {
      const dist = Math.abs(pc - scanX);
      const alpha = Math.exp(-(dist * dist) / (2 * sigma * sigma));
      if (alpha > 0.1) {
        const charIdx = Math.floor(pc / 2);
        const dc = pc % 2;
        for (let row = 0; row < height; row++) {
          field[charIdx] = setDot(field[charIdx], row, dc);
        }
      }
    }
    return field;
  },
};
