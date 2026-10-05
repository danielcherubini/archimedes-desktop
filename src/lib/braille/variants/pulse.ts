import type { VariantConfig } from "../types";
import { setDot, createFieldBuffer } from "../math";

export const pulse: VariantConfig = {
  totalFrames: 23,
  interval: 60,
  gridSize: [4, 4],
  compute: (frame, _totalFrames, width, height, _ctx) => {
    const period = 900;
    const t = frame * 40;
    const scale = 1 + 0.06 * Math.sin((2 * Math.PI * t) / period);
    const field = createFieldBuffer(width);
    const centerX = (width * 2 - 1) / 2;
    const centerY = (height - 1) / 2;

    for (let pc = 0; pc < width * 2; pc++) {
      for (let row = 0; row < height; row++) {
        const dx = (pc - centerX) / scale;
        const dy = (row - centerY) / scale;
        const dist = Math.sqrt(dx * dx + dy * dy);
        const maxDist = Math.sqrt(centerX * centerX + centerY * centerY);
        const ringWidth = 0.8;
        const ringPos = ((Math.sin(((2 * Math.PI * t) / period) * 2) + 1) / 2) * maxDist;

        if (Math.abs(dist - ringPos) < ringWidth) {
          const charIdx = Math.floor(pc / 2);
          const dc = pc % 2;
          field[charIdx] = setDot(field[charIdx], row, dc);
        }
      }
    }
    return field;
  },
};
