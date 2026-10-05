import type { VariantConfig } from "../types";
import { setDot, createFieldBuffer } from "../math";

export const equalizer: VariantConfig = {
  totalFrames: 90,
  interval: 40,
  gridSize: [5, 4],
  compute: (frame, totalFrames, width, height, _ctx) => {
    const field = createFieldBuffer(width);
    const progress = frame / totalFrames;
    const pixelCols = width * 2;
    const t = progress * Math.PI * 2;

    for (let pc = 0; pc < pixelCols; pc++) {
      const seed = pc * 1.618033988749895;
      const freq1 = 3 + (seed % 3);
      const freq2 = 5 + ((seed * 1.3) % 4);
      const phase1 = seed * 0.7;
      const phase2 = seed * 1.3;
      const amp1 = 0.6 + ((seed * 0.17) % 0.4);
      const amp2 = 0.4 + ((seed * 0.23) % 0.3);

      const wave1 = Math.sin(t * freq1 + phase1) * amp1;
      const wave2 = Math.sin(t * freq2 + phase2) * amp2;
      const wave3 = Math.sin(t * 7 + seed) * 0.2;

      const combined = wave1 + wave2 + wave3;
      const normalized = (combined + amp1 + amp2 + 0.2) / (2 * (amp1 + amp2 + 0.2));
      const fillHeight = Math.max(0, Math.min(height, Math.floor(normalized * height)));

      const charIdx = Math.floor(pc / 2);
      const dc = pc % 2;

      for (let row = height - 1; row >= height - fillHeight; row--) {
        if (row >= 0 && row < height) {
          field[charIdx] = setDot(field[charIdx], row, dc);
        }
      }
    }

    return field;
  },
};
