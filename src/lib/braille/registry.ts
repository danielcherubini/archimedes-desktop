import type { VariantConfig } from "./types";
import { pendulum } from "./variants/pendulum";
import { compress } from "./variants/compress";
import { sort } from "./variants/sort";
import { breathe } from "./variants/breathe";
import { pulse } from "./variants/pulse";
import { waveRows } from "./variants/wave-rows";
import { snake } from "./variants/snake";
import { orbit } from "./variants/orbit";
import { rain } from "./variants/rain";
import { sparkle } from "./variants/sparkle";
import { checkerboard } from "./variants/checkerboard";
import { columns } from "./variants/columns";
import { cascade } from "./variants/cascade";
import { diagonalSwipe } from "./variants/diagonal-swipe";
import { scan } from "./variants/scan";
import { fillSweep } from "./variants/fill-sweep";
import { helix } from "./variants/helix";
import { braille } from "./variants/braille";
import { phaseShift } from "./variants/phase-shift";
import { reflectedRipple } from "./variants/reflected-ripple";
import { equalizer } from "./variants/equalizer";
import { chase } from "./variants/chase";
import { bars } from "./variants/bars";
import { marquee } from "./variants/marquee";
import { typing } from "./variants/typing";
import { spiral } from "./variants/spiral";

/**
 * The variant registry — assembles the per-variant configs into the map the frame
 * generator looks up by camelCase variant name. Key order mirrors the historical
 * inline `VARIANT_CONFIGS` blob in `src/lib/braille-loader.ts`.
 */
export const VARIANT_CONFIGS: Record<string, VariantConfig> = {
  pendulum,
  compress,
  sort,
  breathe,
  pulse,
  waveRows,
  snake,
  orbit,
  rain,
  sparkle,
  checkerboard,
  columns,
  cascade,
  diagonalSwipe,
  scan,
  fillSweep,
  helix,
  braille,
  phaseShift,
  reflectedRipple,
  equalizer,
  chase,
  bars,
  marquee,
  typing,
  spiral,
};
