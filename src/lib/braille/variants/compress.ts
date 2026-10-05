import type { VariantConfig } from "../types";
import { setDot, createFieldBuffer } from "../math";

export const compress: VariantConfig = {
  totalFrames: 100,
  interval: 40,
  gridSize: [5, 5],
  compute: (frame, totalFrames, width, height, ctx) => {
    const progress = frame / totalFrames;
    const sieveThreshold = Math.max(0.1, 1 - progress * 1.2);
    const squeeze = Math.min(1, progress / 0.85);
    const activeWidth = Math.max(1, width * 2 * (1 - squeeze * 0.95));
    const field = createFieldBuffer(width);

    for (let pc = 0; pc < width * 2; pc++) {
      const mappedPc = (pc / (width * 2)) * activeWidth;
      if (mappedPc >= activeWidth) continue;
      const targetPc = Math.round(mappedPc);
      if (targetPc >= width * 2) continue;
      const charIdx = Math.floor(targetPc / 2);
      const dc = targetPc % 2;

      for (let row = 0; row < height; row++) {
        const importanceIdx = pc * height + row;
        if (ctx.importance[importanceIdx] < sieveThreshold) {
          field[charIdx] = setDot(field[charIdx], row, dc);
        }
      }
    }
    return field;
  },
};
