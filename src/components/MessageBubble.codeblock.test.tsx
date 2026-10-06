import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, it, expect, beforeAll, afterEach, vi } from "vitest";
import MessageBubble, { shikiThemeFor } from "./MessageBubble";
import { Message } from "../store/sessions";
import { useSettings } from "../store/settings";
import type { AppSettings } from "../lib/tauri";

/**
 * The fenced-code block's header (the ZCode model: file-type glyph + hue +
 * language label on the left, a copy button on the right), asserted through
 * the REAL `ReactMarkdown` renderer and the REAL Shiki highlighter (no mock —
 * Shiki works under jsdom; the highlight resolves asynchronously out of a
 * `useEffect`, hence every query is `findBy*`).
 *
 * Two things are pinned here that are easy to regress:
 * - The header is present for EVERY fence shape, including the no-language
 *   and unknown-language cases (the block never changes shape, so the panel
 *   never "pops" between highlighted and fallback rendering).
 * - `tsx` / `jsx` fences are genuinely highlighted. They used to throw inside
 *   Shiki (`Language not found`) because the shared highlighter only loaded a
 *   small `langs` allowlist; the `catch` swallowed it and the most common
 *   fences in a coding agent's output silently rendered UNHIGHLIGHTED.
 * - The SYNTAX theme derives from the palette (ADR 0027) — see the
 *   "the syntax theme follows the palette" group at the bottom.
 */
function renderAgentText(text: string) {
  const message: Message = { kind: "agent-text", messageId: "m1", text, at: 1 };
  return render(<MessageBubble message={message} />);
}

/**
 * A full `AppSettings` document to seed the settings store with (the shape
 * `ChatStream.test.tsx`'s `SETTINGS_FIXTURE` uses). `palette` is a REQUIRED
 * member of the type (ADR 0027), so the fixture has to carry it rather than
 * omit it — the tests below overwrite just that one field.
 */
const SETTINGS_FIXTURE: AppSettings = {
  theme: "dark",
  palette: null,
  paneLayout: {},
  defaultTrustNewSpaces: false,
  defaultModel: null,
  defaultThinkingLevel: null,
  enabledTools: [],
  providers: [],
  mcpServers: {},
  font: { sizePx: 14, uiFamily: null, codeFamily: null },
  defaultThinkingLevels: {},
  subagentModels: {},
  spinnerStyle: null,
};

/** Seed the settings store with the fixture under the given palette. */
function seedPalette(palette: AppSettings["palette"]) {
  useSettings.getState().setSettings({ ...SETTINGS_FIXTURE, palette });
}

beforeAll(() => {
  Object.defineProperty(window, "matchMedia", {
    writable: true,
    value: vi.fn().mockImplementation((query) => ({
      matches: false,
      media: query,
      onchange: null,
      addListener: vi.fn(),
      removeListener: vi.fn(),
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
      dispatchEvent: vi.fn(),
    })),
  });
});

afterEach(() => {
  vi.restoreAllMocks();
  // The copy-button tests below run on fake timers; the timer state is
  // module-global, so it must be handed back even when one of them fails
  // halfway through or the later groups in this file would inherit them.
  vi.useRealTimers();
  // The palette tests seed the settings store; the OTHER tests in this file
  // assert the default (zai / `github-dark`) paper, so the store MUST go back
  // to "nothing loaded" between tests or they would inherit Dracula and pass
  // for the wrong reason.
  useSettings.setState({ settings: null, loaded: false });
});

describe("CodeBlock header (the ZCode code-block model)", () => {
  it("gives a tsx fence the typescript glyph, the `tsx` label and a copy button", async () => {
    renderAgentText(
      "```tsx\nexport function App() {\n  return <p>hi</p>;\n}\n```",
    );

    const header = await screen.findByTestId("code-block-header");
    const label = screen.getByTestId("code-block-lang");
    expect(label.textContent).toBe("tsx");
    // The glyph carries the SAME descriptor class as a `.tsx` FileChip in a
    // tool row, which is the whole point of reusing `fileIconFor`.
    const svg = header.querySelector("svg");
    expect(svg?.getAttribute("class") ?? "").toContain("text-file-ts");
    expect(screen.getByRole("button", { name: "Copy code" })).toBeTruthy();
  });

  it("gives a json fence the json glyph and hue", async () => {
    renderAgentText('```json\n{\n  "a": 1\n}\n```');

    const header = await screen.findByTestId("code-block-header");
    expect(screen.getByTestId("code-block-lang").textContent).toBe("json");
    expect(header.querySelector("svg")?.getAttribute("class") ?? "").toContain(
      "text-file-json",
    );
  });

  it("labels a language-less fence `text` with the neutral default glyph", async () => {
    // MUST be multi-line: a single-line fence with no language is routed to
    // the INLINE `<code>` chip by the `code()` renderer, so a one-liner would
    // fail for a completely unrelated reason.
    renderAgentText("```\nline one\nline two\n```");

    const header = await screen.findByTestId("code-block-header");
    expect(screen.getByTestId("code-block-lang").textContent).toBe("text");
    expect(header.querySelector("svg")?.getAttribute("class") ?? "").toContain(
      "text-foreground-subtlest",
    );
  });

  it("shows an unknown language verbatim (lowercased) instead of falling back to `text`", async () => {
    renderAgentText("```bogus\nnot a real language\n```");

    const header = await screen.findByTestId("code-block-header");
    // The label is the trimmed lowercased lang whenever lang is non-empty —
    // only an ABSENT language means "text".
    expect(screen.getByTestId("code-block-lang").textContent).toBe("bogus");
    expect(header.querySelector("svg")?.getAttribute("class") ?? "").toContain(
      "text-foreground-subtlest",
    );
  });

  it("copies the fence source and confirms visibly", async () => {
    const source = "const x = 1;\nconst y = 2;";
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText },
      configurable: true,
    });

    renderAgentText("```ts\n" + source + "\n```");

    const button = await screen.findByRole("button", { name: "Copy code" });
    fireEvent.click(button);

    expect(await screen.findByRole("button", { name: "Copied" })).toBeTruthy();
    expect(writeText).toHaveBeenCalledWith(source);
  });

  it("renders the highlighted body (Shiki token spans + its own paper), not the fallback", async () => {
    renderAgentText(
      "```tsx\nexport const flag: boolean = true;\n```",
    );

    // The header resolves in the same effect that resolves the highlight, so
    // once it is there the html is settled: either highlighted or fallback.
    const header = await screen.findByTestId("code-block-header");
    const body = (header.parentElement?.lastElementChild ?? null) as HTMLElement | null;
    expect(body).not.toBeNull();

    const pre = body!.querySelector("pre");
    expect(pre).not.toBeNull();
    // Shiki emits an INLINE `background-color` on the `<pre>` (which is why
    // the body must NOT fight it with a Tailwind background class), and token
    // `<span>`s inside. Neither exists in the fallback `<pre><code>` path.
    expect(pre!.getAttribute("style") ?? "").toContain("background-color");
    expect(pre!.querySelector("span")).not.toBeNull();
    expect(body!.className).not.toContain("bg-card");
    // The header rides the PANEL step of the elevation ladder, not the card.
    // `fileIconContrast.test.ts` grades every descriptor on that surface and
    // is what makes it a legibility requirement rather than a preference.
    expect(header.className).toContain("bg-panel");
    expect(header.className).not.toContain("bg-card");
  });

  it("highlights jsx too (both alias grammars are loaded)", async () => {
    renderAgentText("```jsx\nconst App = () => <p>hi</p>;\n```");

    const header = await screen.findByTestId("code-block-header");
    expect(screen.getByTestId("code-block-lang").textContent).toBe("jsx");
    const body = (header.parentElement?.lastElementChild ?? null) as HTMLElement | null;
    const pre = body?.querySelector("pre") ?? null;
    expect(pre?.getAttribute("style") ?? "").toContain("background-color");
    expect(pre?.querySelector("span")).not.toBeNull();
  });
});

/**
 * The two body branches must be ONE block of code.
 *
 * The component's own doc-comment promises the fence "must never change shape
 * between highlighted and fallback rendering", and the shape claim has two
 * halves: the HEADER (pinned above) and the BODY's inset. The body padding used
 * to live in two separate class literals — `overflow-x-auto p-3 font-mono
 * text-sm` on the fallback `<pre>` and `overflow-x-auto font-mono text-sm` on
 * the highlighted `<div>` — so the ~99% of fences that highlight were flush
 * against the panel edge while the rare fallback was inset, and NOTHING in the
 * file could see it: the only body assertion was `not.toContain("bg-card")`,
 * which is about the background axis and stays green while the padding axis
 * diverges. Shiki emits no padding of its own (its `<pre class="shiki …">`
 * carries only an inline colour) and no rule in `index.css` pads `pre`,
 * `.shiki` or `code`, so the missing class was the whole inset.
 *
 * So these assert PARITY (the two class strings are identical) rather than a
 * second list of expected classes — a parity assertion cannot be satisfied by
 * editing one literal, which is exactly how the divergence happened.
 */
describe("CodeBlock body: the highlighted and fallback branches cannot diverge", () => {
  /** The body element of the (single) rendered fence. */
  const bodyOf = async () => {
    const header = await screen.findByTestId("code-block-header");
    return (header.parentElement?.lastElementChild ?? null) as HTMLElement | null;
  };

  it("insets the highlighted body and the fallback body IDENTICALLY", async () => {
    const highlighted = renderAgentText(
      "```tsx\nexport const flag: boolean = true;\n```",
    );
    const hotBody = await bodyOf();
    expect(hotBody).not.toBeNull();
    // Genuinely highlighted, or the parity below would be comparing the
    // fallback with itself.
    expect(hotBody!.querySelector("pre.shiki")).not.toBeNull();
    const hotClasses = hotBody!.className;
    highlighted.unmount();

    const fallback = renderAgentText("```bogus\nnot a real language\n```");
    const coldBody = await bodyOf();
    expect(coldBody).not.toBeNull();
    expect(coldBody!.querySelector("pre.shiki")).toBeNull();

    // The padding, named — because "identical" alone is satisfiable by two
    // identically-flush branches (which is what shipped).
    const coldClasses = coldBody!.className;
    fallback.unmount();

    expect(coldClasses.split(" ")).toContain("p-3");
    expect(hotClasses.split(" ")).toEqual(coldClasses.split(" "));
  });
});

/**
 * FIX: the header's label and the highlighter must be fed the SAME language.
 *
 * The label is `lang.trim().toLowerCase()`, but `codeToHtml` was passed the RAW
 * `lang`. Shiki resolves language aliases case-sensitively, so a
 * \`\`\`TypeScript\`\`\` or \`\`\`JSON\`\`\` fence threw `Language not found`
 * inside the `try`, the `catch` set `html` to `null`, and the bubble rendered
 * the UNHIGHLIGHTED fallback while the header calmly displayed the lowercased
 * label — i.e. the fence looked like a highlighted `typescript` block and was
 * silently plain text. Agents emit capitalised fences constantly, so this was
 * not a corner case.
 */
describe("CodeBlock language casing (a ```TypeScript fence is highlighted)", () => {
  it.each(["TypeScript", "JSON", "python"])(
    "renders a `%s` fence genuinely highlighted, with the lowercased label",
    async (lang) => {
      renderAgentText(`\`\`\`${lang}\nconst x = 1;\n\`\`\``);

      const header = await screen.findByTestId("code-block-header");
      expect(screen.getByTestId("code-block-lang").textContent).toBe(
        lang.toLowerCase(),
      );
      const body = (header.parentElement?.lastElementChild ??
        null) as HTMLElement | null;
      expect(body).not.toBeNull();
      // The same technique as the tsx/jsx tests: Shiki's OWN `<pre>` (a
      // descendant — in the fallback branch the `<pre>` IS the body element and
      // there is no `shiki` class anywhere) with its inline paper colour and
      // token spans. A regression to the catch-and-fall-back path fails HERE.
      const pre = body!.querySelector("pre.shiki");
      expect(pre, "the fence rendered the unhighlighted fallback").not.toBeNull();
      expect(pre!.getAttribute("style") ?? "").toContain("background-color");
      expect(pre!.querySelector("span")).not.toBeNull();
      expect(document.querySelector("pre.shiki")).not.toBeNull();
    },
  );

  it("highlights a fence whose language is padded with whitespace", async () => {
    // `label` trims; the highlighter call must trim too, or `" ts "` throws.
    renderAgentText("``` ts\nconst x = 1;\n```");
    const header = await screen.findByTestId("code-block-header");
    expect(screen.getByTestId("code-block-lang").textContent).toBe("ts");
    // And the header's GLYPH is the trimmed descriptor, i.e. the header and the
    // body agree about which language this is — the same agreement the fix is
    // about, read from the other side.
    expect(header.querySelector("svg")?.getAttribute("class") ?? "").toContain(
      "text-file-ts",
    );
    expect(header.parentElement?.querySelector("pre.shiki")).not.toBeNull();
  });
});

/**
 * (ADR 0027) The palette axis decides the SYNTAX theme — there is
 * deliberately no user-facing code-theme picker, so the derivation rule is
 * pinned here in two layers: the pure mapper (no DOM), and the paper colour
 * Shiki actually paints on the `<pre>`.
 */
describe("shikiThemeFor — the syntax theme derives from the palette (ADR 0027)", () => {
  it("maps the dracula palette to Shiki's `dracula` theme", () => {
    expect(shikiThemeFor("dracula")).toBe("dracula");
  });

  it("maps the zai palette to `github-dark` (the unchanged existing behaviour)", () => {
    expect(shikiThemeFor("zai")).toBe("github-dark");
  });

  it("treats an unloaded (`null`) settings palette as zai → `github-dark`", () => {
    expect(shikiThemeFor(null)).toBe("github-dark");
  });

  it("treats an absent (`undefined`) palette as zai → `github-dark`", () => {
    expect(shikiThemeFor(undefined)).toBe("github-dark");
  });
});

describe("the syntax theme follows the palette", () => {
  /**
   * The inline `background-color` Shiki writes on its own `<pre>` — i.e. the
   * theme paper that actually rendered. `pre.shiki` because react-markdown
   * wraps the fence in its OWN `<pre>` (unstyled), which would otherwise be
   * the first match. Shiki's themes spell their hexes with their own casing,
   * which is reproduced verbatim here rather than normalised: `#282A36` is
   * Dracula's paper, `#24292e` is GitHub dark's.
   */
  const renderedPaper = () =>
    document.querySelector("pre.shiki")?.getAttribute("style") ?? "";

  it("paints Dracula's paper on code fences under the Dracula palette", async () => {
    seedPalette("dracula");
    renderAgentText("```tsx\nexport const flag: boolean = true;\n```");

    const header = await screen.findByTestId("code-block-header");
    const body = (header.parentElement
      ?.lastElementChild ?? null) as HTMLElement | null;
    const pre = body?.querySelector("pre") ?? null;
    expect(pre).not.toBeNull();
    expect(renderedPaper()).toContain("#282A36");
    // Still a REAL highlight (token spans), not the plain fallback `<pre>`.
    expect(pre!.querySelector("span")).not.toBeNull();
  });

  it("paints GitHub dark's paper when the palette is unset (the zai default)", async () => {
    // `null` is what the backend stores for "zai" — the default must not need
    // an explicit `"zai"` to keep the paper it has always had.
    seedPalette(null);
    renderAgentText("```tsx\nexport const flag: boolean = true;\n```");

    const header = await screen.findByTestId("code-block-header");
    const body = (header.parentElement
      ?.lastElementChild ?? null) as HTMLElement | null;
    expect(body?.querySelector("pre")).not.toBeNull();
    expect(renderedPaper()).toContain("#24292e");
  });

  it("re-highlights an ALREADY-rendered fence the moment the palette changes (live, no remount)", async () => {
    // The point of putting `palette` in the effect's dependency array: the
    // SAME mounted bubble (same message object, so the memoized bubble is not
    // even re-rendered by its parent) must re-colour itself when the setting
    // changes. A test that only checked the mapper would not catch a missing
    // dependency here.
    seedPalette(null);
    renderAgentText("```tsx\nexport const flag: boolean = true;\n```");

    await waitFor(() => expect(renderedPaper()).toContain("#24292e"));

    act(() => {
      seedPalette("dracula");
    });

    await waitFor(() => expect(renderedPaper()).toContain("#282A36"));
    // And the token spans survive the re-highlight (it is Shiki output, not a
    // fallback that happens to have a dark background).
    expect(
      document.querySelector("pre.shiki")?.querySelector("span"),
    ).not.toBeNull();
  });
});

/**
 * The copy button's TWO stated lifetime claims, neither of which any test used
 * to make:
 *
 *  - "Copy → confirm for 1.5s". Deleting the `window.setTimeout(...)` entirely
 *    (so the button sticks on "Copied" forever) left all 14 tests green, because
 *    the only copy test asserted the FORWARD transition — which a never-resetting
 *    timer performs perfectly.
 *  - "the unmount cleanup clears it — no `setState` on an unmounted bubble".
 *    Deleting the whole `useEffect` cleanup left all 14 tests green too.
 *
 * Both are pinned on `vi.getTimerCount()`, the number of timers actually pending
 * in the faked clock. That is what makes the unmount assertion mean something:
 * React 19 REMOVED the "can't perform a state update on an unmounted component"
 * warning, so spying `console.error` for it would assert nothing at all — the
 * warning it looks for can never fire. A leftover timer, by contrast, is a fact
 * about the tree, and it is checked in both directions (1 while the confirmation
 * is showing, 0 after unmount) so the assertion cannot pass merely because the
 * component never scheduled a timer.
 */
describe("CodeBlock copy button: the 1.5s reset and the unmount cleanup", () => {
  /** Install a clipboard that resolves, so `handleCopy`'s promise continuation
   *  runs and the state update lands. */
  function stubClipboard() {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText },
      configurable: true,
    });
    return writeText;
  }

  /** Click copy and let the `writeText().then(...)` continuation run. */
  async function clickCopy(button: HTMLElement) {
    fireEvent.click(button);
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });
  }

  const copyLabel = () =>
    screen.getByRole("button").getAttribute("aria-label");

  it("resets from Copied back to Copy code after 1.5s", async () => {
    stubClipboard();
    renderAgentText("```ts\nconst x = 1;\n```");
    const button = await screen.findByRole("button", { name: "Copy code" });

    vi.useFakeTimers();
    await clickCopy(button);
    expect(copyLabel()).toBe("Copied");
    // The claim being tested is the RESET, so pin that a reset is scheduled and
    // has not fired yet — 1.5s is the number the component states.
    expect(vi.getTimerCount()).toBe(1);

    await act(async () => {
      vi.advanceTimersByTime(1499);
      await Promise.resolve();
    });
    expect(copyLabel()).toBe("Copied");

    await act(async () => {
      vi.advanceTimersByTime(1);
      await Promise.resolve();
    });
    expect(copyLabel()).toBe("Copy code");
    expect(vi.getTimerCount()).toBe(0);
  });

  it("restarts the 1.5s window on a second click instead of double-scheduling", async () => {
    // The component clears the pending timer before scheduling a new one, so the
    // confirmation is 1.5s from the LAST click. Without the `clearTimeout` the
    // first timer would still be in flight and the label would flip back early,
    // mid-confirmation — and the timer count here is what distinguishes the two.
    stubClipboard();
    renderAgentText("```ts\nconst x = 1;\n```");
    const button = await screen.findByRole("button", { name: "Copy code" });

    vi.useFakeTimers();
    await clickCopy(button);
    await act(async () => {
      vi.advanceTimersByTime(1000);
      await Promise.resolve();
    });
    // Still confirming 500ms in; the icon swapped, so re-query the same node.
    expect(copyLabel()).toBe("Copied");
    await act(async () => {
      await clickCopy(screen.getByRole("button"));
    });
    expect(vi.getTimerCount(), "a second click must replace the pending reset, not add one").toBe(1);
    await act(async () => {
      vi.advanceTimersByTime(1000);
      await Promise.resolve();
    });
    expect(copyLabel()).toBe("Copied");
    await act(async () => {
      vi.advanceTimersByTime(500);
      await Promise.resolve();
    });
    expect(copyLabel()).toBe("Copy code");
  });

  it("clears the pending reset timer when the bubble unmounts mid-confirmation", async () => {
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    stubClipboard();
    const { unmount } = renderAgentText("```ts\nconst x = 1;\n```");
    const button = await screen.findByRole("button", { name: "Copy code" });

    vi.useFakeTimers();
    await clickCopy(button);
    expect(copyLabel()).toBe("Copied");
    // Non-vacuity guard: a timer really is in flight, so the 0 below is the
    // CLEANUP removing it rather than the component never having scheduled one.
    expect(vi.getTimerCount()).toBe(1);

    act(() => {
      unmount();
    });
    expect(vi.getTimerCount(), "the unmount cleanup must clear the pending reset").toBe(
      0,
    );

    // And firing whatever is left must not touch the dead tree. React 19 no
    // longer warns about this, hence the timer assertion above is the real gate;
    // this one is a cheap backstop against any future warning or error.
    await act(async () => {
      vi.advanceTimersByTime(5000);
      await Promise.resolve();
    });
    const unmountWarnings = errors.mock.calls
      .flat()
      .filter((a) => typeof a === "string" && /unmounted|setState|memory leak/i.test(a));
    expect(unmountWarnings).toEqual([]);
  });
});
