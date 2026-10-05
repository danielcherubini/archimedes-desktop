import type { VariantConfig } from "../types";
import { setDot, createFieldBuffer } from "../math";

export const diagonalSwipe: VariantConfig = {
  totalFrames: 60,
  interval: 30,
  gridSize: [3, 6],
  compute: (frame, totalFrames, width, height, _ctx) => {
    const field = createFieldBuffer(width);
    const pixelCols = width * 2;

    const maxDiag = pixelCols - 1 + (height - 1);
    const cycleFrame = frame % totalFrames;
    const clearFrames = Math.max(2, Math.floor(totalFrames / 2));
    const fillFrames = Math.max(2, totalFrames - clearFrames);

    const clearPhase = cycleFrame < clearFrames;
    const localFrame = clearPhase ? cycleFrame : cycleFrame - clearFrames;
    const localTotal = (clearPhase ? clearFrames : fillFrames) - 1;
    const phaseProgress = localTotal > 0 ? localFrame / localTotal : 1;
    const sweepFront = phaseProgress * (maxDiag + 1);

    for (let pc = 0; pc < pixelCols; pc++) {
      for (let row = 0; row < height; row++) {
        const diag = pc + row;
        const show = clearPhase ? diag >= sweepFront : diag < sweepFront;
        if (show) {
          const charIdx = Math.floor(pc / 2);
          const dc = pc % 2;
          field[charIdx] = setDot(field[charIdx], row, dc);
        }
      }
    }
    return field;
  },
};
