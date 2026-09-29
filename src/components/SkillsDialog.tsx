import { useState } from "react";
import type { SkillInfo } from "../lib/tauri";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "./ui/dialog";
import { Input } from "./ui/input";

/**
 * The Skills modal (ZCode parity: the left pane has NO skill list — the
 * skills UI is a searchable list). One row per (filtered) skill: name
 * (primary) + description (secondary, one-line truncated, full text in a
 * `title` tooltip); user-level rows carry a subtle "global" cue. Clicking
 * a row inserts `$name ` into the composer via the
 * `archimedes:insert-skill` CustomEvent (the composer listens) and closes
 * the dialog. The search filters by case-insensitive NAME substring (v1:
 * name only — the same policy as the composer's skill picker).
 *
 * PROP-DRIVEN: the catalog fetch lives in `useSkillCatalog` in
 * `SpacesList` (the modal and the composer share the same hook key → one
 * fetch); the dialog receives the rows.
 */
export default function SkillsDialog({
  skills,
  onClose,
}: {
  skills: SkillInfo[];
  onClose: () => void;
}) {
  const [query, setQuery] = useState("");
  const needle = query.trim().toLowerCase();
  const filtered =
    needle === ""
      ? skills
      : skills.filter((s) => s.name.toLowerCase().includes(needle));

  /**
   * Insert `$<name> ` into the composer (a decoupled `window`
   * CustomEvent — the composer lives in a different subtree) and close
   * the dialog. The name is LOWERCASED (the case policy: the inserted
   * token must be expandable, and the mention regex is lowercase-only).
   */
  const insertSkill = (name: string) => {
    window.dispatchEvent(
      new CustomEvent("archimedes:insert-skill", {
        detail: "$" + name.toLowerCase() + " ",
      }),
    );
    onClose();
  };

  return (
    <Dialog open onOpenChange={(nextOpen) => !nextOpen && onClose()}>
      <DialogContent className="max-h-[70vh]">
        <DialogHeader>
          <DialogTitle>Skills</DialogTitle>
        </DialogHeader>
        <div className="flex max-h-[calc(70vh-7.5rem)] flex-col gap-3">
          <Input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search skills…"
          />
          {skills.length === 0 ? (
            <p className="p-3 text-ui-sm text-foreground-subtlest">
              No skills found.
            </p>
          ) : filtered.length === 0 ? (
            <p className="p-3 text-ui-sm text-foreground-subtlest">
              No skills match "{query}".
            </p>
          ) : (
            <div className="flex flex-col gap-0.5 overflow-y-auto">
              {filtered.map((skill) => (
                <div
                  key={skill.path}
                  role="button"
                  tabIndex={0}
                  onClick={() => insertSkill(skill.name)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") insertSkill(skill.name);
                  }}
                  className="group flex cursor-pointer flex-col gap-0.5 rounded-lg px-2.5 py-1 hover:bg-surface-hover"
                >
                  <div className="flex items-center gap-2">
                    <span className="flex-1 truncate text-ui-base">
                      {skill.name}
                    </span>
                    {skill.scope === "user" && (
                      <span className="text-ui-xs text-foreground-subtlest">
                        global
                      </span>
                    )}
                  </div>
                  {skill.description !== "" && (
                    <span
                      className="truncate text-ui-sm text-foreground-subtle"
                      title={skill.description}
                    >
                      {skill.description}
                    </span>
                  )}
                </div>
              ))}
            </div>
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}
