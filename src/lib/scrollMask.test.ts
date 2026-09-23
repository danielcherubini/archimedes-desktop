import { describe, it, expect } from 'vitest';
import {
  resolveVerticalScrollMaskState,
  getVerticalScrollMaskStyle,
  EMPTY_SCROLL_MASK_STATE,
} from './scrollMask';

describe('scrollMask', () => {
  it('resolveVerticalScrollMaskState', () => {
    // scrollHeight <= clientHeight
    expect(resolveVerticalScrollMaskState({ scrollHeight: 100, clientHeight: 100, scrollTop: 0 })).toEqual(EMPTY_SCROLL_MASK_STATE);
    expect(resolveVerticalScrollMaskState({ scrollHeight: 99, clientHeight: 100, scrollTop: 0 })).toEqual(EMPTY_SCROLL_MASK_STATE);
    
    // scrollTop = 0, scrollable
    expect(resolveVerticalScrollMaskState({ scrollHeight: 200, clientHeight: 100, scrollTop: 0 })).toEqual({ showTop: false, showBottom: true });
    
    // scrollTop past top threshold, short of bottom
    expect(resolveVerticalScrollMaskState({ scrollHeight: 200, clientHeight: 100, scrollTop: 50 })).toEqual({ showTop: true, showBottom: true });
    
    // scrollTop at bottom
    expect(resolveVerticalScrollMaskState({ scrollHeight: 200, clientHeight: 100, scrollTop: 100 })).toEqual({ showTop: true, showBottom: false });
  });

  it('getVerticalScrollMaskStyle', () => {
    expect(getVerticalScrollMaskStyle(EMPTY_SCROLL_MASK_STATE)).toBeUndefined();
    
    // showBottom only
    expect(getVerticalScrollMaskStyle({ showTop: false, showBottom: true })).toEqual({
      maskRepeat: "no-repeat",
      maskSize: "100% 100%",
      maskImage: "linear-gradient(to bottom, black 0px, black 24px, black calc(100% - 24px), transparent 100%)",
    });
    
    // showTop only
    expect(getVerticalScrollMaskStyle({ showTop: true, showBottom: false })).toEqual({
      maskRepeat: "no-repeat",
      maskSize: "100% 100%",
      maskImage: "linear-gradient(to bottom, transparent 0px, black 24px, black calc(100% - 24px), black 100%)",
    });
    
    // Both
    expect(getVerticalScrollMaskStyle({ showTop: true, showBottom: true })).toEqual({
      maskRepeat: "no-repeat",
      maskSize: "100% 100%",
      maskImage: "linear-gradient(to bottom, transparent 0px, black 24px, black calc(100% - 24px), transparent 100%)",
    });
  });
});
