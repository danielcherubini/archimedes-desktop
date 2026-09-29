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
});
