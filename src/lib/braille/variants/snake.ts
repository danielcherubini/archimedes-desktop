import type { VariantConfig } from "../types";
import { setDot, createFieldBuffer } from "../math";

export const snake: VariantConfig = {
  totalFrames: 25,
  interval: 80,
  gridSize: [2, 4],
  compute: (frame, totalFrames, width, height, _ctx) => {
    const field = createFieldBuffer(width);
    const pixelCols = width * 2;

    // Build serpentine path: left→right on even rows, right→left on odd rows
    const path: Array<{ pc: number; row: number }> = [];
    for (let row = 0; row < height; row++) {
      if (row % 2 === 0) {
        for (let pc = 0; pc < pixelCols; pc++) path.push({ pc, row });
      } else {
        for (let pc = pixelCols - 1; pc >= 0; pc--) path.push({ pc, row });
      }
    }

    const progress = frame / totalFrames;
    const trailingCells = 3;
    const headPos = Math.floor(progress * path.length) % path.length;
    const deadGap = 0;

    const drawPathDot = (idx: number) => {
      const { pc, row } = path[idx];
      const charIdx = Math.floor(pc / 2);
      const dc = pc % 2;
      field[charIdx] = setDot(field[charIdx], row, dc);
    };

    drawPathDot(headPos);

    for (let i = deadGap + 1; i < deadGap + 1 + trailingCells; i++) {
      const idx = (headPos - i + path.length) % path.length;
      drawPathDot(idx);
    }

    return field;
  },
};
