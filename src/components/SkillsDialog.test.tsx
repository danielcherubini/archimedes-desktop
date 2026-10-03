import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { SkillInfo } from "../lib/tauri";
import SkillsDialog from "./SkillsDialog";

// The dialog is PROP-DRIVEN (it does not fetch — the fetch lives in
// `useSkillCatalog` in `SpacesList`, covered by `useSkillCatalog.test.ts`),
// so the catalog rows are passed as a prop and no `listSkills` mock is
// needed (the only tauri import is the type).
function skill(overrides: Partial<SkillInfo> = {}): SkillInfo {
  return {
    name: "alpha",
    description: "Alpha skill",
    path: "/p/.agents/skills/alpha/SKILL.md",
    dir: "/p/.agents/skills/alpha",
    scope: "space",
    body: "B",
    ...overrides,
  };
}

describe("SkillsDialog", () => {
  it("renders_one_row_per_skill_with_name_and_description", async () => {
    render(
      <SkillsDialog
        skills={[
          skill(),
          skill({
            name: "beta",
            description: "Beta skill",
            path: "/p/.agents/skills/beta/SKILL.md",
            dir: "/p/.agents/skills/beta",
          }),
        ]}
        onClose={() => {}}
      />,
    );
    await screen.findByText("alpha");
    await screen.findByText("Alpha skill");
    await screen.findByText("beta");
    await screen.findByText("Beta skill");
  });

  it("user_scope_rows_carry_the_global_cue", async () => {
    // TWO SEPARATE renders — a single render with both skills would make
    // a global `queryByText("global")` ambiguous (the user row's badge
    // matches).
    const user = render(
      <SkillsDialog skills={[skill({ scope: "user" })]} onClose={() => {}} />,
    );
    await screen.findByText("global");
    user.unmount();
    const space = render(
      <SkillsDialog skills={[skill({ scope: "space" })]} onClose={() => {}} />,
    );
    expect(screen.queryByText("global")).toBeNull();
    space.unmount();
  });

  it("clicking_a_row_dispatches_the_insert_event_and_closes", async () => {
    const events: CustomEvent[] = [];
    const listener = (e: Event) => {
      events.push(e as CustomEvent);
    };
    window.addEventListener("archimedes:insert-skill", listener);
    const onClose = vi.fn();
    try {
      // Uppercase name: the case policy lowercases the INSERTED token
      // (the mention regex is lowercase-only).
      render(<SkillsDialog skills={[skill({ name: "Alpha" })]} onClose={onClose} />);
      const row = (await screen.findByText("Alpha")).closest(
        '[role="button"]',
      );
      expect(row).not.toBeNull();
      fireEvent.click(row!);
      const dispatched = events.find(
        (e) => e.type === "archimedes:insert-skill",
      );
      expect(dispatched).toBeDefined();
      expect(dispatched!.detail).toBe("$alpha ");
      // The dialog closes on insert (one pick, one action).
      expect(onClose).toHaveBeenCalledTimes(1);
    } finally {
      window.removeEventListener("archimedes:insert-skill", listener);
    }
  });

  it("renders_the_empty_state", async () => {
    render(<SkillsDialog skills={[]} onClose={() => {}} />);
    await screen.findByText("No skills found.");
  });

  it("the_search_filters_by_name", async () => {
    render(
      <SkillsDialog
        skills={[
          skill({
            name: "debug",
            description: "Debug skill",
            path: "/p/.agents/skills/debug/SKILL.md",
            dir: "/p/.agents/skills/debug",
          }),
          skill({
            name: "beta",
            description: "Beta skill",
            path: "/p/.agents/skills/beta/SKILL.md",
            dir: "/p/.agents/skills/beta",
          }),
        ]}
        onClose={() => {}}
      />,
    );
    const search = screen.getByPlaceholderText("Search skills…");
    // Case-insensitive NAME substring: `debug` stays, `beta` is filtered out.
    fireEvent.change(search, { target: { value: "de" } });
    expect(screen.getByText("debug")).toBeTruthy();
    expect(screen.queryByText("beta")).toBeNull();
    // Nothing matches → no rows, and a muted no-match cue.
    fireEvent.change(search, { target: { value: "zzz" } });
    expect(screen.queryByText("debug")).toBeNull();
    expect(screen.queryByText("beta")).toBeNull();
    expect(screen.getByText('No skills match "zzz".')).toBeTruthy();
  });

  it("the dialog content is bounded (max-w-3xl — not the full-width default)", async () => {
    render(<SkillsDialog skills={[skill()]} onClose={() => {}} />);
    const title = await screen.findByText("Skills");
    const content = title.closest("[data-slot=dialog-content]");
    expect(content?.className).toContain("max-w-3xl");
  });

  it("the truncation chain is bounded (min-w-0 on the wrapper, the scroll container, and the row — the text cannot escape the right edge)", async () => {
    render(
      <SkillsDialog
        skills={[
          skill({
            name: "long-desc",
            description: "x".repeat(400),
            path: "/p/.agents/skills/long-desc/SKILL.md",
            dir: "/p/.agents/skills/long-desc",
          }),
        ]}
        onClose={() => {}}
      />,
    );
    const desc = await screen.findByText("x".repeat(400));
    // The leaf spans: `min-w-0` lets them shrink below their (nowrap)
    // content width so `truncate` can clip them.
    expect(desc.className).toContain("min-w-0");
    expect(desc.className).toContain("truncate");
    const name = screen.getByText("long-desc");
    expect(name.className).toContain("min-w-0");
    // AND the whole item chain between the dialog box and the spans must
    // be shrinkable: the wrapper (a grid item of the dialog), the scroll
    // container (a flex item of the wrapper), and the row (a flex item of
    // the scroll container). Without `min-w-0` at EACH level, the
    // `min-width: auto` floor (the min-content width) lets the nowrap
    // text push every ancestor past the dialog's right edge.
    const row = desc.closest("[role=button]");
    expect(row?.className).toContain("min-w-0");
    const scroll = row?.parentElement;
    expect(scroll?.className).toContain("min-w-0");
    expect(scroll?.className).toContain("overflow-y-auto");
    // Right padding keeps the row content clear of the scrollbar (which
    // sits in the container's right edge and would otherwise overlap the
    // rows' right side).
    expect(scroll?.className).toContain("pr-2");
    const wrapper = scroll?.parentElement;
    expect(wrapper?.className).toContain("min-w-0");
  });
});
