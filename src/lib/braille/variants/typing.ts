import type { VariantConfig } from "../types";
import { setDot, createFieldBuffer } from "../math";

export const typing: VariantConfig = {
  totalFrames: 80,
  interval: 50,
  gridSize: [4, 3],
  compute: (frame, totalFrames, width, height, _ctx) => {
    const field = createFieldBuffer(width);
    const totalCells = width * height;

    const cycleLength = totalCells + 6;
    const currentCell = Math.floor((frame / totalFrames) * cycleLength * 2) % cycleLength;

    for (let cell = 0; cell < currentCell && cell < totalCells; cell++) {
      const charIdx = cell % width;
      const row = Math.floor(cell / width);
      field[charIdx] = setDot(field[charIdx], row, 0);
      field[charIdx] = setDot(field[charIdx], row, 1);
    }

    const cursorPos = Math.min(currentCell, totalCells - 1);
    const blinkOn = Math.floor(frame / 1) % 2 === 0;

    if (blinkOn && cursorPos >= 0) {
      const cursorCharIdx = cursorPos % width;
      const cursorRow = Math.floor(cursorPos / width);
      field[cursorCharIdx] = setDot(field[cursorCharIdx], cursorRow, 0);
      field[cursorCharIdx] = setDot(field[cursorCharIdx], cursorRow, 1);
    }

    return field;
  },
};
