import type { VariantConfig } from "../types";
import { clamp, setDot, createFieldBuffer } from "../math";

export const rain: VariantConfig = {
  totalFrames: 90,
  interval: 40,
  gridSize: [5, 5],
  compute: (frame, _totalFrames, width, height, ctx) => {
    const field = createFieldBuffer(width);
    const pixelCols = width * 2;
    const t = frame * 40;
    let activeDrops = 0;

    for (let pc = 0; pc < pixelCols; pc++) {
      const rand = ctx.colRandom[pc] ?? 0;

      const period = 1200 + rand * 1000;
      const cyclePos = t / period + rand * 0.91 + pc * 0.07;
      const cycleIndex = Math.floor(cyclePos);
      const phase = cyclePos - cycleIndex;

      const seedA = cycleIndex * 173 + pc * 37 + Math.floor(rand * 1009);
      const noiseA = Math.sin(seedA * 12.9898) * 43758.5453;
      const rollA = noiseA - Math.floor(noiseA);

      const seedB = cycleIndex * 257 + pc * 61 + Math.floor(rand * 881);
      const noiseB = Math.sin(seedB * 78.233) * 12345.6789;
      const rollB = noiseB - Math.floor(noiseB);

      const seedC = cycleIndex * 97 + pc * 149 + Math.floor(rand * 733);
      const noiseC = Math.sin(seedC * 39.3467) * 31337.4242;
      const rollC = noiseC - Math.floor(noiseC);

      const missChance = 0.02 + rand * 0.08;
      if (rollA < missChance) continue;

      const spawnDelay = 0.0 + rollB * 0.2;
      const fallDuration = 0.48 + rollC * 0.42;
      const endPhase = spawnDelay + fallDuration;

      if (phase < spawnDelay || phase > endPhase) continue;

      const localPhase = (phase - spawnDelay) / fallDuration;
      const gravityCurve = 1.6 + rollC * 1.2;
      const accelerated = Math.pow(localPhase, gravityCurve);

      const midWeight = Math.max(0, 1 - Math.abs(localPhase - 0.5) * 2);
      const wobbleSeed = cycleIndex * 0.73 + pc * 1.31 + rand * 4.7;
      const midWobble = Math.sin(wobbleSeed + localPhase * Math.PI * 6) * 0.1 * midWeight;

      const y = Math.floor((accelerated + midWobble) * (height + 1)) - 1;
      if (y < 0 || y >= height) continue;

      const charIdx = Math.floor(pc / 2);
      const dc = pc % 2;
      field[charIdx] = setDot(field[charIdx], y, dc);
      activeDrops++;
    }

    if (activeDrops === 0) {
      const fallbackPos = (t / 1600) % 1;
      const fallbackPc = Math.floor(fallbackPos * pixelCols) % pixelCols;
      const fallbackPhase = fallbackPos * (height + 1);
      const fallbackY = clamp(Math.floor(fallbackPhase), 0, height - 1);
      const fallbackCharIdx = Math.floor(fallbackPc / 2);
      const fallbackDc = fallbackPc % 2;
      field[fallbackCharIdx] = setDot(field[fallbackCharIdx], fallbackY, fallbackDc);
    }

    return field;
  },
};
