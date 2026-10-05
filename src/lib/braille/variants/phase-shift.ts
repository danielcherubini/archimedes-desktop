import type { VariantConfig } from "../types";
import { setDot, createFieldBuffer } from "../math";

export const phaseShift: VariantConfig = {
  totalFrames: 60,
  interval: 40,
  gridSize: [5, 5],
  compute: (frame, totalFrames, width, height, _ctx) => {
    const field = createFieldBuffer(width);
    const progress = totalFrames > 1 ? frame / (totalFrames - 1) : 0;
    const drawableHeight = Math.min(height, 4);
    const centerX = (width * 2 - 1) / 2;
    const centerY = (drawableHeight - 1) / 2;

    const phasePosition = (progress * 8) % 4;

    for (let pc = 0; pc < width * 2; pc++) {
      for (let row = 0; row < drawableHeight; row++) {
        const isLeft = pc < centerX;
        const isTop = row < centerY;

        let quadrantIndex = 0;
        if (isTop && isLeft) quadrantIndex = 0;
        else if (isTop && !isLeft) quadrantIndex = 1;
        else if (!isTop && !isLeft) quadrantIndex = 2;
        else quadrantIndex = 3;

        const phaseDeltaRaw = Math.abs(phasePosition - quadrantIndex);
        const phaseDelta = Math.min(phaseDeltaRaw, 4 - phaseDeltaRaw);

        const primary = Math.max(0, 1 - phaseDelta / 0.7);
        const secondary = Math.max(0, 1 - phaseDelta / 1.35) * 0.45;
        const intensity = primary + secondary;

        if (intensity > 0.42) {
          const charIdx = Math.floor(pc / 2);
          const dc = pc % 2;
          field[charIdx] = setDot(field[charIdx], row, dc);
        }
      }
    }
    return field;
  },
};
