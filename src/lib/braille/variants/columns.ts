import type { VariantConfig } from "../types";
import { setDot, createFieldBuffer } from "../math";

export const columns: VariantConfig = {
  totalFrames: 48,
  interval: 40,
  gridSize: [4, 4],
  compute: (frame, totalFrames, width, height, _ctx) => {
    const field = createFieldBuffer(width);
    const pixelCols = width * 2;
    const stepsPerColumn = height + 1;
    const totalSteps = pixelCols * stepsPerColumn;

    const progress = frame / totalFrames;
    const step = Math.floor(progress * totalSteps) % totalSteps;
    const activePc = Math.floor(step / stepsPerColumn);
    const activeFill = step % stepsPerColumn;

    const fillColumnToTop = (pc: number) => {
      const charIdx = Math.floor(pc / 2);
      const dc = pc % 2;
      for (let row = 0; row < height; row++) {
        field[charIdx] = setDot(field[charIdx], row, dc);
      }
    };

    const fillColumnBottomUp = (pc: number, count: number) => {
      const charIdx = Math.floor(pc / 2);
      const dc = pc % 2;
      const dotsToFill = Math.max(0, Math.min(height, count));
      for (let i = 0; i < dotsToFill; i++) {
        const row = height - 1 - i;
        field[charIdx] = setDot(field[charIdx], row, dc);
      }
    };

    for (let pc = 0; pc < pixelCols; pc++) {
      if (pc < activePc) {
        fillColumnToTop(pc);
        continue;
      }

      if (pc === activePc) {
        fillColumnBottomUp(pc, activeFill);
      }
    }

    return field;
  },
};
