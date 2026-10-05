import type { VariantConfig } from "../types";
import { setDot, createFieldBuffer } from "../math";

export const waveRows: VariantConfig = {
  totalFrames: 20,
  interval: 40,
  gridSize: [4, 4],
  compute: (frame, totalFrames, width, height, _ctx) => {
    const progress = frame / totalFrames;
    const field = createFieldBuffer(width);
    const pixelCols = width * 2;

    const basePhase = progress * Math.PI * 2;
    const colPhaseStep = (Math.PI * 2) / Math.max(2, pixelCols);
    const bandWidth = 0.9;

    for (let pc = 0; pc < pixelCols; pc++) {
      const colWave = Math.sin(basePhase + pc * colPhaseStep);
      const centerRow = ((colWave + 1) / 2) * (height - 1);

      for (let row = 0; row < height; row++) {
        const dist = Math.abs(row - centerRow);
        if (dist <= bandWidth) {
          const charIdx = Math.floor(pc / 2);
          const dc = pc % 2;
          field[charIdx] = setDot(field[charIdx], row, dc);
        }
      }
    }
    return field;
  },
};
