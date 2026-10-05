import type { VariantConfig } from "../types";
import { setDot, createFieldBuffer } from "../math";

export const checkerboard: VariantConfig = {
  totalFrames: 60,
  interval: 40,
  gridSize: [4, 4],
  compute: (frame, totalFrames, width, height, _ctx) => {
    const drawableHeight = Math.min(height, 4);
    const phaseFrames = Math.max(2, Math.floor(totalFrames / 8));
    const phase = Math.floor(frame / phaseFrames) % 2;
    const field = createFieldBuffer(width);

    for (let pc = 0; pc < width * 2; pc++) {
      for (let row = 0; row < drawableHeight; row++) {
        // True checkerboard: alternate both X (pc) and Y (row) positions
        if ((pc + row) % 2 === phase) {
          const charIdx = Math.floor(pc / 2);
          const dc = pc % 2;
          field[charIdx] = setDot(field[charIdx], row, dc);
        }
      }
    }
    return field;
  },
};
