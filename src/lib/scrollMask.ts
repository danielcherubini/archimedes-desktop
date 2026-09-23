import type { CSSProperties } from "react";

export interface ScrollMetrics {
  scrollTop: number;
  scrollHeight: number;
  clientHeight: number;
}

export interface ScrollMaskState {
  showTop: boolean;
  showBottom: boolean;
}

export const EMPTY_SCROLL_MASK_STATE: ScrollMaskState = {
  showTop: false,
  showBottom: false,
};

export function resolveVerticalScrollMaskState({
  scrollTop,
  scrollHeight,
  clientHeight,
}: ScrollMetrics): ScrollMaskState {
  const isScrollable = scrollHeight > clientHeight;
  if (!isScrollable) {
    return EMPTY_SCROLL_MASK_STATE;
  }

  const showTop = scrollTop > 1;
  const showBottom = scrollTop < scrollHeight - clientHeight - 1;

  return { showTop, showBottom };
}

export function getVerticalScrollMaskStyle({
  showTop,
  showBottom,
}: ScrollMaskState): CSSProperties | undefined {
  if (!showTop && !showBottom) {
    return undefined;
  }

  const topStop = showTop ? "transparent 0px" : "black 0px";
  const bottomStop = showBottom ? "transparent 100%" : "black 100%";

  return {
    maskRepeat: "no-repeat",
    maskSize: "100% 100%",
    maskImage: `linear-gradient(to bottom, ${topStop}, black 24px, black calc(100% - 24px), ${bottomStop})`,
  };
}
