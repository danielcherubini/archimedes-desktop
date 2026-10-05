import type { PrecomputeContext, VariantConfig } from "../types";
import { setDot, createFieldBuffer } from "../math";

export const braille: VariantConfig = {
  totalFrames: 60,
  interval: 40,
  gridSize: [4, 4],
  compute: (frame: number, totalFrames: number, width: number, height: number, _ctx: PrecomputeContext) => {
    const progress = frame / totalFrames;
    const field = createFieldBuffer(width);

    // Braille pattern positions within one character
    const braillePath: Array<{ row: number; dc: number }> = [
      { row: 0, dc: 0 }, // top-left
      { row: 0, dc: 1 }, // top-right
      { row: 1, dc: 1 }, // right-top
      { row: 2, dc: 1 }, // right-bottom
      { row: 3, dc: 1 }, // bottom-right
      { row: 3, dc: 0 }, // bottom-left
      { row: 2, dc: 0 }, // left-bottom
      { row: 1, dc: 0 }, // left-top
    ];

    // 2 moving dots (opposite positions) with 2-dot trails each
    const lead1Index = Math.floor(progress * braillePath.length) % braillePath.length;
    const lead2Index = Math.floor((progress + 0.5) * braillePath.length) % braillePath.length;

    // Only use first character
    const charIdx = 0;

    // First moving dot with 2-dot trail
    for (let i = 0; i < 2; i++) {
      const idx = (lead1Index - i + braillePath.length) % braillePath.length;
      const pos = braillePath[idx];
      if (pos.row < height) {
        field[charIdx] = setDot(field[charIdx], pos.row, pos.dc);
      }
    }

    // Second moving dot with 2-dot trail (opposite)
    for (let i = 0; i < 2; i++) {
      const idx = (lead2Index - i + braillePath.length) % braillePath.length;
      const pos = braillePath[idx];
      if (pos.row < height) {
        field[charIdx] = setDot(field[charIdx], pos.row, pos.dc);
      }
    }

    return field;
  },
};
