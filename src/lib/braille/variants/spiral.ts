import type { VariantConfig } from "../types";
import { setDot, createFieldBuffer } from "../math";

export const spiral: VariantConfig = {
  totalFrames: 60,
  interval: 40,
  gridSize: [5, 5],
  compute: (frame, totalFrames, width, height, _ctx) => {
    const field = createFieldBuffer(width);
    const progress = frame / totalFrames;
    const pixelCols = width * 2;
    const drawableHeight = Math.min(height, 4);
    const centerX = (pixelCols - 1) / 2;
    const centerY = (drawableHeight - 1) / 2;

    const arms = 3;
    const armLength = Math.max(pixelCols, drawableHeight) / 2;
    const rotationOffset = progress * Math.PI * 4;

    for (let arm = 0; arm < arms; arm++) {
      const armAngle = (arm / arms) * Math.PI * 2 + rotationOffset;

      for (let r = 0; r < armLength; r++) {
        const t = r / armLength;
        const spiralAngle = armAngle + t * Math.PI * 1.5;
        const x = centerX + Math.cos(spiralAngle) * r * 0.8;
        const y = centerY + Math.sin(spiralAngle) * r * 0.8;

        const pc = Math.round(x);
        const row = Math.round(y);

        if (pc >= 0 && pc < pixelCols && row >= 0 && row < drawableHeight) {
          const charIdx = Math.floor(pc / 2);
          const dc = pc % 2;
          field[charIdx] = setDot(field[charIdx], row, dc);
        }
      }
    }

    return field;
  },
};
