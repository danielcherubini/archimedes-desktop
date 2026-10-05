/**
 * Shared shapes for the braille spinner engine: the per-variant config
 * (`VariantConfig`) implemented by every file in `braille/variants/`, and the
 * deterministic precomputed noise/context buffers handed to each compute fn.
 */

export type VariantConfig = {
  totalFrames: number;
  interval: number;
  gridSize: [number, number];
  compute: (frame: number, totalFrames: number, width: number, height: number, context: PrecomputeContext) => number[];
};

export type PrecomputeContext = {
  importance: number[];
  shuffled: number[];
  target: number[];
  colRandom: number[];
};
