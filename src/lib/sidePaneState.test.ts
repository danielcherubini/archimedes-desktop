import { beforeEach, describe, expect, it } from "vitest";
import {
  getSidePaneCollapsed,
  setSidePaneCollapsed,
  subscribeSidePane,
} from "./sidePaneState";

beforeEach(() => {
  // Reset the module's in-memory flag to the default (false), then clear
  // the persisted value (the module is the single owner of the key).
  setSidePaneCollapsed(false);
  localStorage.clear();
});

describe("sidePaneState", () => {
  it("round-trips the collapsed flag through get/set", () => {
    expect(getSidePaneCollapsed()).toBe(false);
    setSidePaneCollapsed(true);
    expect(getSidePaneCollapsed()).toBe(true);
    setSidePaneCollapsed(false);
    expect(getSidePaneCollapsed()).toBe(false);
  });

  it("writes the persisted value itself (the single persistence owner)", () => {
    setSidePaneCollapsed(true);
    expect(localStorage.getItem("side-pane-collapse")).toBe("true");
    setSidePaneCollapsed(false);
    expect(localStorage.getItem("side-pane-collapse")).toBe("false");
  });

  it("fires subscribers when the flag flips, and stops after unsubscribing", () => {
    const seen: boolean[] = [];
    const unsubscribe = subscribeSidePane(() => {
      seen.push(getSidePaneCollapsed());
    });
    setSidePaneCollapsed(true);
    expect(seen).toEqual([true]);
    setSidePaneCollapsed(false);
    expect(seen).toEqual([true, false]);
    unsubscribe();
    setSidePaneCollapsed(true);
    // Unsubscribed: no further notification.
    expect(seen).toEqual([true, false]);
  });
});
