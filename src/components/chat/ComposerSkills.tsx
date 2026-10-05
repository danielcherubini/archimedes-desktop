import type { SkillInfo } from "../../lib/tauri";

/**
 * The composer's `$`-trigger skill picker (the floating row list above the
 * textarea).
 *
 * Props-only: the `useSkillCatalog` hook, the `picker` state, the
 * `filtered`/`activeIndex` derivations and `selectSkill` all stay in
 * `ChatStream` — the `picker` state is coordinated with the textarea's
 * `onChange` / `onKeyDown` (the token re-evaluation, the arrow/Enter/Tab/
 * Escape branches) and the send path, so it cannot move here. Extracted
 * verbatim from `ChatStream` (identical DOM); renders nothing while closed.
 */
export default function ComposerSkills({
  open,
  filtered,
  activeIndex,
  onSelect,
}: {
  open: boolean;
  filtered: SkillInfo[];
  activeIndex: number;
  onSelect: (skill: SkillInfo) => void;
}) {
  return (
    <>
      {open && filtered.length > 0 && (
        <div
          data-testid="skill-picker"
          className="absolute left-3 right-3 -top-2 z-10 -translate-y-full rounded-lg border border-input-border bg-input p-1 shadow-lg"
        >
          {filtered.map((s, i) => (
            <button
              key={s.name}
              type="button"
              onMouseDown={(e) => {
                e.preventDefault();
                onSelect(s);
              }}
              className={`flex w-full flex-col gap-0.5 rounded-md px-2 py-1 text-left ${i === activeIndex ? "bg-surface-hover" : ""}`}
            >
              <span className="text-ui-base">{s.name}</span>
              {s.description !== "" && (
                <span
                  className="truncate text-ui-sm text-foreground-subtle"
                  title={s.description}
                >
                  {s.description}
                </span>
              )}
            </button>
          ))}
        </div>
      )}
    </>
  );
}
