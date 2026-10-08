import { describe, expect, it, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  within,
} from "@testing-library/react";
import SessionRail, {
  type SessionRailAction,
  type SessionRailRow,
} from "./SessionRail";

const row = (over: Partial<SessionRailRow> & { key: string }): SessionRailRow => ({
  status: "stored",
  active: false,
  ...over,
});

/**
 * The left rail is the sidebar's session list reduced to icons, for the 40px
 * collapsed sliver. Same grammar as `TodoRail` (see its doc for why the
 * geometry is CSS and why the strip CLIPS instead of scrolling), so this file
 * asserts the STATES and the geometry that keeps it clear of the sidebar's own
 * footer toggle.
 */
describe("SessionRail (the session list as icons, for the collapsed left rail)", () => {
  it("renders one row per session, in the list's order, and NO text", () => {
    const { container } = render(
      <SessionRail
        rows={[
          row({ key: "s1" }),
          row({ key: "s2", status: "running" }),
          row({ key: "s3", status: "waiting" }),
        ]}
      />,
    );
    expect(screen.getAllByTestId("session-row")).toHaveLength(3);
    // ORDER is the list's order (live first, then stored — the sidebar does
    // NOT re-sort). The rail is a POSITIONAL map of the list: the Nth mark is
    // the Nth row, which is the only reason a dotless column means anything.
    const rows = [...container.querySelectorAll("[data-testid='session-row']")];
    expect(rows[1]!.querySelector("svg")!.getAttribute("class")).toContain("animate-spin");
    expect(rows[2]!.querySelector("[data-dot]")).toBeTruthy();    expect(container.textContent).toBe("");
  });

  it("draws a SPINNER for a running session — the same glyph the open row uses", () => {
    render(<SessionRail rows={[row({ key: "s1", status: "running" })]} />);
    const mark = screen.getByTestId("session-mark");
    // One glyph for "this session is working" in the rail AND the list (the
    // open row's leading slot is the same `Spinner`), so the two views of the
    // same fact cannot look like two different facts.
    expect(mark.getAttribute("role")).toBe("status");
    expect(mark.getAttribute("class")).toContain("animate-spin");
    // No folder behind it: the row IS the activity, and 12px cannot hold both.
    expect(screen.getAllByTestId("session-mark")).toHaveLength(1);
  });

  it("tells the four states apart — spinner / warning dot / live dot / bare dot", () => {
    render(
      <SessionRail
        rows={[
          row({ key: "run", status: "running" }),
          row({ key: "wait", status: "waiting" }),
          row({ key: "live", status: "live" }),
          row({ key: "stored", status: "stored" }),
        ]}
      />,
    );
    const [run, wait, live, stored] = screen.getAllByTestId("session-row");
    // Running: the spinner.
    expect(run!.querySelector('[role="status"]')).toBeTruthy();
    // Waiting (a pending permission / ask / password): the warning dot — the
    // app's own attention cue, the same `bg-warning` the toggle dots and the
    // board's `◉` carry. The attribute is the STATUS, so a test can name the
    // state rather than the colour it happens to paint with.
    const waiting = wait!.querySelector('[data-dot="waiting"]');
    expect(waiting).toBeTruthy();
    expect(waiting!.className).toContain("bg-warning");
    expect(wait!.querySelector('[role="status"]')).toBeNull();
    // Live and idle: the filled live dot.
    expect(live!.querySelector('[data-dot="live"]')).toBeTruthy();
    // Stored: a bare, dimmed dot — the session exists, nothing is happening.
    expect(stored!.querySelector('[data-dot="stored"]')).toBeTruthy();
  });

  it("marks the ACTIVE session, by weight rather than a new colour", () => {
    render(<SessionRail rows={[row({ key: "a", active: true }), row({ key: "b" })]} />);
    const [a, b] = screen.getAllByTestId("session-mark");
    expect(a!.className).not.toBe(b!.className);
    // The open list marks the active row with `bg-selected` (a fill); a 12px
    // dot has no room for a fill, so it takes the brighter INK instead — and
    // both values are existing tokens, not a rail-only colour.
    expect(a!.className).toContain("text-foreground");
    expect(a!.className).not.toContain("text-foreground-subtlest");
    expect(b!.className).toContain("text-foreground-subtlest");
  });

  it("clips instead of scrolling, and keeps the strip clear of the footer toggle", () => {
    render(
      <SessionRail rows={Array.from({ length: 40 }, (_, i) => row({ key: `s${i}` }))} />,
    );
    const marks = screen.getByTestId("session-rail-marks");
    const frame = screen.getByTestId("session-rail");
    expect(marks.className).toContain("overflow-hidden");
    expect(frame.className).toContain("overflow-hidden");
    for (const el of [marks, frame]) {
      expect(el.className).not.toMatch(/overflow-[a-z]*-(auto|scroll)/);
    }
    // Top-anchored: an overflowing flex column CENTERS its overflow when
    // justified to the middle, which would cut the FIRST sessions off the top.
    expect(marks.className).toContain("justify-start");
    // Rows share the height with a floor and a ceiling (see `TodoRail`).
    const rows = screen.getAllByTestId("session-row");
    expect(rows).toHaveLength(40);
    for (const r of rows.slice(0, 3)) {
      expect(r.className).toContain("flex-1");
      expect(r.className).toMatch(/min-h-/);
      expect(r.className).toMatch(/max-h-/);
    }
    // The sidebar's footer toggle is `p-2` + `size-6` = 32px of box, so the
    // strip must stop above that or the last mark lands under the toggle.
    const bottom = frame.className.match(/\bbottom-(\d+)/);
    expect(bottom, "the rail is not offset from the bottom").toBeTruthy();
    expect(Number(bottom![1]) * 4).toBeGreaterThanOrEqual(32);
    // The MARKS are a readout and must not steal the toggle's click; the
    // ACTIONS are buttons and must not be click-through. One `pointer-events`
    // rule per block, because they have opposite jobs in the same 40px column.
    expect(marks.className).toContain("pointer-events-none");
    // The FRAME carries no such class: it also holds the action buttons, and a
    // `pointer-events-none` inherited from it would make them click-through
    // (the actions tests assert the other half).
    expect(frame.className).not.toContain("pointer-events-none");
  });

  it("summarises itself for the accessible name (the marks carry no text)", () => {
    render(
      <SessionRail
        rows={[
          row({ key: "a", status: "running" }),
          row({ key: "b", status: "waiting" }),
          row({ key: "c" }),
        ]}
      />,
    );
    // Counts, not titles: the rail is positional (the Nth mark is the Nth row)
    // and the titles live in the list and the transcript.
    expect(
      screen.getByRole("img", { name: "Sessions 3, 1 running, 1 waiting" }),
    ).toBeTruthy();
  });

  it("omits zero counts from the summary (a plain list reads as a plain list)", () => {
    render(<SessionRail rows={[row({ key: "a" }), row({ key: "b" })]} />);
    expect(screen.getByRole("img", { name: "Sessions 2" })).toBeTruthy();
  });

  it("renders nothing for an empty list (an empty rail is just the rail)", () => {
    const { container } = render(<SessionRail rows={[]} />);
    expect(container.textContent).toBe("");
    expect(screen.queryByTestId("session-rail")).toBeNull();
  });

  // -- The action strip (the collapsed rail's other job: the pane's four
  // -- commands, so collapsing the sidebar does not cost you the commands) --

  const actions: SessionRailAction[] = [
    { id: "open", label: "Open Space", icon: <i />, onClick: vi.fn() },
    { id: "new", label: "New Session", icon: <i />, onClick: vi.fn() },
    { id: "skills", label: "Skills", icon: <i />, onClick: vi.fn() },
  ];
  const settings: SessionRailAction = {
    id: "settings",
    label: "Settings",
    icon: <i />,
    onClick: vi.fn(),
  };
  // Explicit props, never a spread: a spread of an optional `rows` would let
  // the component receive `rows={undefined}` and read `.length` off it.
  const someRows = [row({ key: "s1" }), row({ key: "s2" })];
  const renderRail = ({
    rows: r = someRows,
    actions: a,
    tailActions,
  }: {
    rows?: SessionRailRow[];
    actions?: SessionRailAction[];
    tailActions?: SessionRailAction[];
  }) => render(<SessionRail rows={r} actions={a} tailActions={tailActions} />);

  it("renders the actions ABOVE the marks, each clickable by name", () => {
    const onClick = vi.fn();
    render(
      <SessionRail
        rows={[row({ key: "s1" }), row({ key: "s2" })]}
        actions={actions.map((a) => ({ ...a, onClick }))}
      />,
    );
    const box = screen.getByTestId("session-rail-actions");
    expect(box.children).toHaveLength(actions.length);
    for (const a of actions) {
      const btn = screen.getByRole("button", { name: a.label });
      // `title` too: the rail has no room for a label, so the hover text is
      // the only thing that says what the icon is.
      expect(btn.getAttribute("title")).toBe(a.label);
      // The actions come FIRST: the marks keep the positional rule (the Nth
      // mark is the Nth row), which only holds if they start at a FIXED offset
      // — a marks-first layout with actions below would shift nothing, but
      // actions-last would put them under the clipped tail.
      expect(box.compareDocumentPosition(screen.getByTestId("session-rail-marks"))).toBe(
        Node.DOCUMENT_POSITION_FOLLOWING,
      );
    }
    fireEvent.click(screen.getByRole("button", { name: "New Session" }));
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it("keeps the marks clickable-free and the actions clickable (opposite jobs, one column)", () => {
    render(<SessionRail rows={[row({ key: "s1" })]} actions={actions} />);
    expect(screen.getByTestId("session-rail-marks").className).toContain("pointer-events-none");
    for (const a of actions) {
      expect(screen.getByRole("button", { name: a.label })).toBeTruthy();
    }
  });

  it("renders no action strip at all when none are given (the readout-only case)", () => {
    render(<SessionRail rows={[row({ key: "s1" })]} />);
    expect(screen.queryByTestId("session-rail-actions")).toBeNull();
    expect(screen.queryByTestId("session-rail-tail-actions")).toBeNull();
  });

  it("shrinks the marks area to whatever the actions leave, and still clips not scrolls", () => {
    render(
      <SessionRail
        rows={Array.from({ length: 30 }, (_, i) => row({ key: `s${i}` }))}
        actions={actions}
      />,
    );
    const marks = screen.getByTestId("session-rail-marks");
    // `flex-1` + `min-h-0`: the marks take the leftover height and are allowed
    // to shrink below their content — without `min-h-0` a flex child's
    // automatic minimum is its content's height, which would push the action
    // strip (and the frame) taller instead of clipping the marks.
    expect(marks.className).toContain("flex-1");
    expect(marks.className).toContain("min-h-0");
    expect(marks.className).toContain("overflow-hidden");
    expect(marks.className).not.toMatch(/overflow-[a-z]*-(auto|scroll)/);
  });

  // -- The TAIL strip: the command that lives at the BOTTOM of the open pane
  //    keeps that position when the pane collapses --
  //
  // `Settings` is the footer's control in the open sidebar, so it belongs at
  // the bottom of the rail too — one 4px gap above the pane's own toggle. A
  // gear that jumped to the TOP of the rail on collapse would read as a
  // different control in a different place: the eye goes to the bottom for
  // settings, as it does in the open pane and in every other pane's footer in
  // this shell (the inspector's gear is bottom-right for the same reason).
  it("puts the tail action BELOW the marks, clickable", () => {
    renderRail({ actions, tailActions: [settings] });
    const kids = [...screen.getByTestId("session-rail").children];
    const head = screen.getByTestId("session-rail-actions");
    const marks = screen.getByTestId("session-rail-marks");
    const tail = screen.getByTestId("session-rail-tail-actions");
    // DOM order IS visual order (the frame is a `flex-col`), so this reads the
    // stack the user sees: commands, marks, then the bottom command.
    expect(kids.indexOf(tail), "tail below the marks").toBeGreaterThan(
      kids.indexOf(marks),
    );
    expect(kids.indexOf(marks), "marks below the head").toBeGreaterThan(
      kids.indexOf(head),
    );
    fireEvent.click(within(tail).getByRole("button", { name: "Settings" }));
    expect(settings.onClick).toHaveBeenCalledTimes(1);
  });

  it("ends the tail strip in the 4px gap ABOVE the pane's own toggle", () => {
    // The footer's toggle is `p-2` + `size-6` = a 32px box sitting 8px off the
    // bottom, so its top edge is 32px up. The frame ends at `bottom-9` (36px),
    // which lands the LAST row in the column exactly 4px above that box — the
    // app's chrome gap, not an invented spacing. `shrink-0` keeps it a real
    // 24px target when a long session list squeezes everything else.
    renderRail({ actions, tailActions: [settings] });
    expect(screen.getByTestId("session-rail").className).toMatch(/\bbottom-9\b/);
    expect(screen.getByTestId("session-rail-tail-actions").className).toMatch(
      /\bshrink-0\b/,
    );
  });

  it("keeps the marks clickable-through with a tail strip present", () => {
    // The tail is the block NEAREST the footer toggle: if `pointer-events-none`
    // leaked from the marks onto it the gear would be dead, and if the marks
    // grew over it the toggle's click would be eaten.
    renderRail({ actions, tailActions: [settings] });
    expect(screen.getByTestId("session-rail-marks").className).toMatch(
      /\bpointer-events-none\b/,
    );
    expect(
      screen.getByTestId("session-rail-tail-actions").className,
    ).not.toMatch(/pointer-events-none/);
  });


  it("pushes the tail strip to the bottom with `mt-auto`, NOT with a sibling", () => {
    // Two ways to anchor the last block of a flex column: give a SIBLING
    // `flex-1` (so it eats the slack), or give the block itself `margin-top:
    // auto`. The sibling route looked natural here because the marks area is
    // already `flex-1` — but it silently breaks the case where that sibling
    // does not exist, i.e. an EMPTY session list: with no marks block nothing
    // ate the slack and the gear floated up into the middle of the rail
    // (rendered it, saw it). `mt-auto` needs no collaborator, so the gear sits
    // at the bottom whether the space has 24 sessions or none, and the marks
    // block stays omitted when empty (an empty `role="img"` reading "Sessions
    // 0" is noise in the a11y tree).
    renderRail({ rows: [], tailActions: [settings] });
    expect(screen.queryByTestId("session-rail-marks")).toBeNull();
    expect(screen.getByTestId("session-rail-tail-actions").className).toMatch(
      /\bmt-auto\b/,
    );
    // And it still works WITH marks — the slack just comes from the list.
    cleanup();
    renderRail({ rows: [row({ key: "s1" })], actions, tailActions: [settings] });
    expect(screen.getByTestId("session-rail-tail-actions").className).toMatch(
      /\bmt-auto\b/,
    );
  });
});
