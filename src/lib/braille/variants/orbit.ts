import type { PrecomputeContext, VariantConfig } from "../types";
import { setDot, createFieldBuffer } from "../math";

export const orbit: VariantConfig = {
  totalFrames: 40,
  interval: 50,
  gridSize: [4, 4],
  compute: (frame: number, totalFrames: number, width: number, height: number, _ctx: PrecomputeContext) => {
    const progress = frame / totalFrames;
    const field = createFieldBuffer(width);

    // Edge positions in clockwise order around one braille character
    const edgePositions: Array<{ row: number; dc: number }> = [
      { row: 0, dc: 0 }, // top-left
      { row: 0, dc: 1 }, // top-right
      { row: 1, dc: 1 }, // right-1
      { row: 2, dc: 1 }, // right-2
      { row: 3, dc: 1 }, // bottom-right
      { row: 3, dc: 0 }, // bottom-left
      { row: 2, dc: 0 }, // left-2
      { row: 1, dc: 0 }, // left-1
    ];

    // 3-dot trail moving clockwise
    const leadPos = Math.floor(progress * edgePositions.length) % edgePositions.length;
    const trailLength = 3;

    // Only use first character
    const charIdx = 0;

    for (let i = 0; i < trailLength; i++) {
      const idx = (leadPos - i + edgePositions.length) % edgePositions.length;
      const pos = edgePositions[idx];

      if (pos.row < height) {
        field[charIdx] = setDot(field[charIdx], pos.row, pos.dc);
      }
    }

    return field;
  },
};
