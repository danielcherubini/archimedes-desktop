import type { VariantConfig } from "../types";
import { clamp, smoothstep, setDot, createFieldBuffer } from "../math";

export const reflectedRipple: VariantConfig = {
  totalFrames: 60,
  interval: 40,
  gridSize: [6, 6],
  compute: (frame, totalFrames, width, height, _ctx) => {
    const field = createFieldBuffer(width);
    const progress = totalFrames > 1 ? frame / (totalFrames - 1) : 0;
    const drawableHeight = Math.min(height, 4);
    const centerX = (width * 2 - 1) / 2;
    const outward = progress < 0.5;
    const local = outward ? progress / 0.5 : (progress - 0.5) / 0.5;
    const localClamped = clamp(local, 0, 1);
    const easedLocal = smoothstep(localClamped) * 0.2 + localClamped * 0.8;
    const radius = outward ? easedLocal * centerX : (1 - easedLocal) * centerX;
    const ringThickness = 1.1;

    for (let pc = 0; pc < width * 2; pc++) {
      const distFromCenter = Math.abs(pc - centerX);
      const bandStrength = Math.max(0, 1 - Math.abs(distFromCenter - radius) / ringThickness);

      if (bandStrength > 0.3) {
        const charIdx = Math.floor(pc / 2);
        const dc = pc % 2;

        for (let row = 0; row < drawableHeight; row++) {
          field[charIdx] = setDot(field[charIdx], row, dc);
        }
      }
    }

    return field;
  },
};
