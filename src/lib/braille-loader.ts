/**
 * Public entry point for the procedural braille spinner engine. The per-variant
 * animation configs live in `src/lib/braille/variants/*` and are assembled by
 * `src/lib/braille/registry.ts`; the shared math helpers live in
 * `src/lib/braille/math.ts`. Consumers keep importing from this module.
 */
import { VARIANT_CONFIGS } from "./braille/registry";
import type { PrecomputeContext } from "./braille/types";
import {
  createFieldBuffer,
  fieldToString,
  seededRandom,
} from "./braille/math";

export { VARIANT_CONFIGS };
export { seededRandom } from "./braille/math";

export const brailleLoaderVariants = [
  "breathe",
  "pulse",
  "orbit",
  "snake",
  "fill-sweep",
  "scan",
  "rain",
  "cascade",
  "checkerboard",
  "columns",
  "wave-rows",
  "diagonal-swipe",
  "sparkle",
  "helix",
  "braille",
  "reflected-ripple",
  "pendulum",
  "compress",
  "sort",
  "equalizer",
  "chase",
  "bars",
  "marquee",
  "typing",
  "spiral",
] as const;

export type BrailleLoaderVariant = (typeof brailleLoaderVariants)[number];
export type BrailleLoaderSpeed = "slow" | "normal" | "fast";

export const speedToDuration: Record<BrailleLoaderSpeed, number> = {
  slow: 3000,
  normal: 2400,
  fast: 1200,
};

const contextCache = new Map<string, PrecomputeContext>();

export function getPrecomputeContext(width: number, height: number): PrecomputeContext {
  const key = `${width}x${height}`;
  let ctx = contextCache.get(key);
  if (!ctx) {
    const pixelCols = width * 2;
    const totalDots = pixelCols * height;

    const rand42 = seededRandom(42);
    const importance = Array.from({ length: totalDots }, () => rand42());

    const rand19 = seededRandom(19);
    const shuffled: number[] = [];
    const target: number[] = [];
    for (let i = 0; i < pixelCols; i++) {
      shuffled.push(rand19() * (height - 1));
      target.push((1 - i / (pixelCols - 1)) * (height - 1));
    }

    const rand123 = seededRandom(123);
    const colRandom: number[] = [];
    for (let pc = 0; pc < pixelCols; pc++) {
      colRandom.push(rand123());
    }

    ctx = {
      importance,
      shuffled,
      target,
      colRandom,
    };
    contextCache.set(key, ctx);
  }
  return ctx;
}

function toCamelCase(str: string): string {
  return str.replace(/-([a-z])/g, (_, letter) => letter.toUpperCase());
}

const frameCache = new Map<string, string[]>();

export function generateFrames(variant: string, width: number, height: number): { frames: string[]; interval: number } {
  const key = `${variant}-${width}x${height}`;
  const cached = frameCache.get(key);
  if (cached) {
    return { frames: cached, interval: VARIANT_CONFIGS[toCamelCase(variant)]?.interval || 40 };
  }

  const config = VARIANT_CONFIGS[toCamelCase(variant)];
  if (!config) {
    return { frames: [fieldToString(createFieldBuffer(width))], interval: 40 };
  }

  const defaultArea = config.gridSize[0] * config.gridSize[1];
  const customArea = width * height;
  const scaleFactor = customArea / defaultArea;
  const clampedScale = Math.max(0.5, Math.min(3, scaleFactor));
  const scaledFrames = Math.round(config.totalFrames * clampedScale);
  const totalFrames = Math.max(30, scaledFrames);

  const context = getPrecomputeContext(width, height);
  const frames: string[] = [];

  for (let frame = 0; frame < totalFrames; frame++) {
    const field = config.compute(frame, totalFrames, width, height, context);
    frames.push(fieldToString(field));
  }

  frameCache.set(key, frames);
  return { frames, interval: config.interval };
}

export function getVariantGridSize(variant: string): [number, number] {
  const config = VARIANT_CONFIGS[toCamelCase(variant)];
  if (config?.gridSize) {
    return config.gridSize;
  }
  return [4, 4];
}

export function normalizeVariant(variant?: string): BrailleLoaderVariant {
  if (!variant) return "breathe";
  return brailleLoaderVariants.includes(variant as BrailleLoaderVariant) ? (variant as BrailleLoaderVariant) : "breathe";
}
