import type { VariantConfig } from "../types";
import { setDot, createFieldBuffer } from "../math";

export const fillSweep: VariantConfig = {
  totalFrames: 80,
  interval: 60,
  gridSize: [4, 4],

  compute: (frame, _totalFrames, width, height, _ctx) => {
    const field = createFieldBuffer(width);

    const framesPerStep = 2;

    const rawStep = frame / framesPerStep;
    const baseStep = Math.floor(rawStep);
    const phase = rawStep - baseStep;

    const maxFill = height;
    const cycle = maxFill * 2;

    const triangle = (s: number) => maxFill - Math.abs((s % cycle) - maxFill);

    const levelA = triangle(baseStep);
    const levelB = triangle(baseStep + 1);

    // ✅ temporal smoothing
    const fillLevel = phase < 0.5 ? levelA : levelB;

    const maxRow = height - 1;

    for (let i = 0; i < fillLevel; i++) {
      const row = maxRow - i;

      for (let charIdx = 0; charIdx < width; charIdx++) {
        field[charIdx] = setDot(field[charIdx], row, 0);
        field[charIdx] = setDot(field[charIdx], row, 1);
      }
    }

    return field;
  },
};
