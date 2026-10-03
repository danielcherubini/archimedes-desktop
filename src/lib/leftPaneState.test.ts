import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  getLeftPaneCollapsed,
  setLeftPaneCollapsed,
  subscribeLeftPane,
} from "./leftPaneState";

beforeEach(() => {
  // Reset the module's in-memory flag to the default (false), then clear
  // the persisted value (the module is the single owner of the key).
  setLeftPaneCollapsed(false);
  localStorage.clear();
});

describe("leftPaneState", () => {
  it("round-trips the collapsed flag through get/set", () => {
    expect(getLeftPaneCollapsed()).toBe(false);
    setLeftPaneCollapsed(true);
    expect(getLeftPaneCollapsed()).toBe(true);
    setLeftPaneCollapsed(false);
    expect(getLeftPaneCollapsed()).toBe(false);
  });

  it("writes the persisted value itself (the single persistence owner)", () => {
    setLeftPaneCollapsed(true);
    expect(localStorage.getItem("left-pane-collapse")).toBe("true");
    setLeftPaneCollapsed(false);
    expect(localStorage.getItem("left-pane-collapse")).toBe("false");
  });

  it("still flips the flag and notifies subscribers when the localStorage write throws (quota/lockdown)", () => {
    const setItem = vi
      .spyOn(Storage.prototype, "setItem")
      .mockImplementation(() => {
        throw new DOMException("quota", "QuotaExceededError");
      });
    try {
      const seen: boolean[] = [];
      const unsubscribe = subscribeLeftPane(() => {
        seen.push(getLeftPaneCollapsed());
      });
      setLeftPaneCollapsed(true);
      expect(getLeftPaneCollapsed()).toBe(true);
      expect(seen).toEqual([true]);
      unsubscribe();
    } finally {
      setItem.mockRestore();
    }
  });

  it("fires subscribers when the flag flips, and stops after unsubscribing", () => {
    const seen: boolean[] = [];
    const unsubscribe = subscribeLeftPane(() => {
      seen.push(getLeftPaneCollapsed());
    });
    setLeftPaneCollapsed(true);
    expect(seen).toEqual([true]);
    setLeftPaneCollapsed(false);
    expect(seen).toEqual([true, false]);
    unsubscribe();
    setLeftPaneCollapsed(true);
    // Unsubscribed: no further notification.
    expect(seen).toEqual([true, false]);
  });
});
