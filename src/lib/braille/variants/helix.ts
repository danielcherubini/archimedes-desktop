import type { VariantConfig } from "../types";
import { clamp, smoothstep, setDot, createFieldBuffer } from "../math";

export const helix: VariantConfig = {
  totalFrames: 64,
  interval: 40,
  gridSize: [5, 5],
  compute: (frame, totalFrames, width, height, _ctx) => {
    const field = createFieldBuffer(width);
    const pixelCols = width * 2;
    const drawableHeight = Math.min(height, 4);
    if (drawableHeight < 2) return field;
    const maxRow = drawableHeight - 1;

    const progress = (frame % totalFrames) / Math.max(1, totalFrames - 1);
    const ramp = smoothstep(progress);
    const speedFactor = 0.7 + ramp * 0.7;
    const scrollSteps = progress * (pixelCols + 8) * speedFactor;
    const shiftA = Math.floor(scrollSteps);
    const shiftB = shiftA + 1;
    const shiftBlend = scrollSteps - shiftA;

    const levelScale = maxRow / 3;
    const mapLevel = (level: number) => clamp(Math.round(level * levelScale), 0, maxRow);

    const chainStates: Array<{ a: number; b: number; bridge: boolean }> = [
      { a: 0, b: 3, bridge: false },
      { a: 1, b: 2, bridge: true },
      { a: 2, b: 1, bridge: true },
      { a: 3, b: 0, bridge: false },
    ];

    const drawDot = (pc: number, row: number) => {
      if (pc < 0 || pc >= pixelCols || row < 0 || row >= drawableHeight) return;
      const charIdx = Math.floor(pc / 2);
      const dc = pc % 2;
      field[charIdx] = setDot(field[charIdx], row, dc);
    };

    for (let pc = 0; pc < pixelCols; pc++) {
      const idxA = (((pc + shiftA) % chainStates.length) + chainStates.length) % chainStates.length;
      const idxB = (((pc + shiftB) % chainStates.length) + chainStates.length) % chainStates.length;

      const columnPhase = (pc + 0.5) / pixelCols;
      const useB = columnPhase < shiftBlend;
      const state = useB ? chainStates[idxB] : chainStates[idxA];

      const rowA = mapLevel(state.a);
      const rowB = mapLevel(state.b);

      drawDot(pc, rowA);
      drawDot(pc, rowB);

      if (state.bridge) {
        const bridge = clamp(Math.round((rowA + rowB) / 2), 0, maxRow);
        drawDot(pc, bridge);
      }
    }

    return field;
  },
};
