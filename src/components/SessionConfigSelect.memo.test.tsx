import { describe, expect, it, vi, beforeAll } from "vitest";
import { fireEvent, render } from "@testing-library/react";
import { createElement, useCallback, useState } from "react";
import type { SessionConfigOption } from "../lib/tauri";

/**
 * Regression test (perf): a live session mounts the config selects in the
 * composer header with pi's FULL model catalog (~600 options). The model
 * option is a DIALOG (`ModelPicker`) — its catalog rows are only mounted
 * while the dialog is open (a closed Radix `Select` would keep ~600 items
 * mounted into a detached DocumentFragment — `Collection` registration —
 * re-rendering them on EVERY parent render; with `draft` state in
 * `ChatStream`, that happened on EVERY KEYSTROKE, costing ~140ms each and
 * making typing lag a full minute behind — the dialog form makes that
 * cost zero at rest).
 *
 * The invariant: when the parent re-renders with a referentially-stable
 * `option` and `onSet` (the composer's typing path), the model option's
 * subtree (`ModelPicker`) must NOT re-render. `SessionConfigSelect` must
 * be memoized, and `onSet` must be a stable `(optionId, value)` callback
 * (not a per-option closure minted per render, which defeats memo).
 */

let pickerRenders = 0;
vi.mock("./ModelPicker", async (importOriginal) => {
  const actual =
    await importOriginal<typeof import("./ModelPicker")>();
  return {
    default: function ProbeModelPicker(
      props: React.ComponentProps<typeof actual.default>,
    ) {
      pickerRenders += 1;
      return createElement(actual.default, props);
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
  it("does not re-render the model picker when the parent re-renders (typing)", async () => {
    pickerRenders = 0;
    const utils = render(<KeystrokeParent option={mockOption} />);
    expect(pickerRenders).toBe(1); // initial mount

    // Simulate keystrokes: parent state changes → parent re-renders with
    // the SAME option reference and an equivalent onSet.
    await utils.findByText("type");
    for (let i = 0; i < 5; i++) {
      fireEvent.click(utils.getByText("type"));
    }

    // The model picker must not have re-rendered beyond the initial mount.
    expect(pickerRenders).toBe(1);
  });

  it("still re-renders when the option object changes (config applied)", async () => {
    pickerRenders = 0;
    const utils = render(
      <KeystrokeParent option={{ ...mockOption, currentValue: "acme/beta" }} />,
    );
    expect(pickerRenders).toBe(1);

    // A NEW option object (the store replaces configOptions on apply):
    utils.rerender(
      <KeystrokeParent
        option={{ ...mockOption, currentValue: "acme/alpha" }}
      />,
    );

    // The model picker re-renders: memo must not swallow real config
    // updates.
    expect(pickerRenders).toBeGreaterThan(1);
  });
});
