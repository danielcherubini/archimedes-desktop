import { describe, expect, it, vi, beforeAll } from "vitest";
import { fireEvent, render } from "@testing-library/react";
import { createElement, useCallback, useState } from "react";
import type { SessionConfigOption } from "../lib/tauri";

/**
 * Regression test (perf): a live session mounts the config selects in the
 * composer header with pi's FULL model catalog (~600 options). Radix keeps
 * closed-select items mounted into a detached DocumentFragment (Collection
 * registration), so every re-render of `SessionConfigSelect` re-renders
 * ~600 invisible `SelectItem`s (~8 fibers each) — with `draft` state in
 * `ChatStream`, that happened on EVERY KEYSTROKE, costing ~140ms each and
 * making typing lag a full minute behind (measured live: 33.5k dev-timer
 * calls per commit, tree 2,135 → 7,052 fibers on connect).
 *
 * The invariant: when the parent re-renders with a referentially-stable
 * `option` and `onSet` (the composer's typing path), the items must NOT
 * re-render. `SessionConfigSelect` must be memoized, and `onSet` must be
 * a stable `(optionId, value)` callback (not a per-option closure minted
 * per render, which defeats memo).
 */

let itemRenders = 0;
vi.mock("../components/ui/select", async (importOriginal) => {
  const actual =
    await importOriginal<typeof import("../components/ui/select")>();
  return {
    ...actual,
    SelectItem: function ProbeSelectItem(
      props: React.ComponentProps<typeof actual.SelectItem>,
    ) {
      itemRenders += 1;
      return createElement(actual.SelectItem, props);
    },
  };
});

// Import AFTER the mock is registered.
import SessionConfigSelect from "./SessionConfigSelect";

beforeAll(() => {
  vi.stubGlobal("matchMedia", (q: string) => ({
    matches: false,
    media: q,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
    addListener: vi.fn(),
    removeListener: vi.fn(),
    dispatchEvent: vi.fn(),
  }));
  Element.prototype.scrollIntoView = vi.fn();
  Element.prototype.hasPointerCapture = vi.fn(() => false);
  Element.prototype.setPointerCapture = vi.fn();
  Element.prototype.releasePointerCapture = vi.fn();
});

const mockOption: SessionConfigOption = {
  id: "model",
  name: "Model",
  type: "select",
  currentValue: "acme/alpha",
  options: [
    { value: "acme/alpha", name: "acme/Alpha" },
    { value: "acme/beta", name: "acme/Beta" },
    { value: "acme/gamma", name: "acme/Gamma" },
  ],
};

/** A parent whose re-render simulates the composer's per-keystroke render. */
function KeystrokeParent({ option }: { option: SessionConfigOption }) {
  const [, setDraft] = useState("");
  // The FIXED ChatStream passes ONE stable callback for both selects
  // (useCallback) — not a per-option closure minted per render.
  const onSet = useCallback(
    async (_optionId: string, _value: string) => {},
    [],
  );
  return (
    <div>
      <button onClick={() => setDraft("x")}>type</button>
      <SessionConfigSelect option={option} onSet={onSet} />
    </div>
  );
}

describe("SessionConfigSelect memoization", () => {
  it("does not re-render its items when the parent re-renders (typing)", async () => {
    itemRenders = 0;
    const utils = render(<KeystrokeParent option={mockOption} />);
    expect(itemRenders).toBe(3); // initial mount: one per option

    // Simulate keystrokes: parent state changes → parent re-renders with
    // the SAME option reference and an equivalent onSet.
    await utils.findByText("type");
    for (let i = 0; i < 5; i++) {
      fireEvent.click(utils.getByText("type"));
    }

    // The items must not have re-rendered beyond the initial mount.
    expect(itemRenders).toBe(3);
  });

  it("still re-renders when the option object changes (config applied)", async () => {
    itemRenders = 0;
    const utils = render(
      <KeystrokeParent option={{ ...mockOption, currentValue: "acme/beta" }} />,
    );
    expect(itemRenders).toBe(3);

    // A NEW option object (the store replaces configOptions on apply):
    utils.rerender(
      <KeystrokeParent
        option={{ ...mockOption, currentValue: "acme/alpha" }}
      />,
    );

    // Items re-render: memo must not swallow real config updates.
    expect(itemRenders).toBeGreaterThan(3);
  });
});
