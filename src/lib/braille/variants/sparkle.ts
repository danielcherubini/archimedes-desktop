import type { VariantConfig } from "../types";
import { clamp, setDot, createFieldBuffer } from "../math";

export const sparkle: VariantConfig = {
  totalFrames: 60,
  interval: 40,
  gridSize: [5, 5],

  compute: (frame, _totalFrames, width, height, _ctx) => {
    const field = createFieldBuffer(width);

    const pixelCols = width * 2;
    const drawableHeight = Math.min(height, 4);

    /* ---------------------------------- */
    /* HASH */
    /* ---------------------------------- */
    const hash = (x: number, y: number, t: number) => {
      let n = x * 374761393 + y * 668265263 + t * 1442695041;
      n = (n ^ (n >> 13)) * 1274126177;
      return ((n ^ (n >> 16)) >>> 0) / 4294967295;
    };

    /* ---------------------------------- */
    /* PARAMETERS */
    /* ---------------------------------- */
    const density = 0.095;
    const lifetime = 4;
    const phase = Math.floor(frame / 2);

    const regionSize = 2;

    for (let row = 0; row < drawableHeight; row++) {
      for (let col = 0; col < pixelCols; col++) {
        let v = hash(col, row, phase);

        /* ---------------------------------- */
        /* ⭐ EDGE COMPENSATION (NEW) */
        /* prevents center bias */
        /* ---------------------------------- */
        const edgeBias =
          0.12 * ((col === 0 || col === pixelCols - 1 ? 1 : 0) + (row === 0 || row === drawableHeight - 1 ? 1 : 0));

        v -= edgeBias;

        if (v > density) continue;

        /* ---------------------------------- */
        /* REGIONAL COMPETITION */
        /* ---------------------------------- */
        const rx = Math.floor(col / regionSize);
        const ry = Math.floor(row / regionSize);

        let winner = true;

        for (let oy = 0; oy < regionSize && winner; oy++) {
          for (let ox = 0; ox < regionSize; ox++) {
            const nx = rx * regionSize + ox;
            const ny = ry * regionSize + oy;

            if (nx === col && ny === row) continue;
            if (nx >= pixelCols || ny >= drawableHeight) continue;

            let nv = hash(nx, ny, phase);

            const nEdgeBias =
              0.12 * ((nx === 0 || nx === pixelCols - 1 ? 1 : 0) + (ny === 0 || ny === drawableHeight - 1 ? 1 : 0));

            nv -= nEdgeBias;

            if (nv < v) {
              winner = false;
              break;
            }
          }
        }

        if (!winner) continue;

        /* ---------------------------------- */
        /* LIFECYCLE */
        /* ---------------------------------- */
        const offset = Math.floor(hash(col, row, 999) * lifetime);

        const age = (frame + offset) % lifetime;

        if (age > 3) continue;

        /* ---------------------------------- */
        /* SHIMMER */
        /* ---------------------------------- */
        let r = row;
        let c = col;

        if (hash(col, row, 777) < 0.18 && age === 1) {
          const dirs = [
            [-1, -1],
            [-1, 0],
            [-1, 1],
            [0, -1],
            [0, 1],
            [1, -1],
            [1, 0],
            [1, 1],
          ];

          const d = dirs[Math.floor(hash(col, row, 555) * dirs.length)];

          r = clamp(r + d[0], 0, drawableHeight - 1);
          c = clamp(c + d[1], 0, pixelCols - 1);
        }

        const charIdx = Math.floor(c / 2);
        const dc = c % 2;

        field[charIdx] = setDot(field[charIdx], r, dc);
      }
    }

    return field;
  },
};
