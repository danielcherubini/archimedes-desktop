import { useMemo } from "react";
import { Check } from "lucide-react";
import { Progress } from "./ui/progress";
import { useBridge, type TodoItem } from "../store/bridge";
import { useSessions } from "../store/sessions";

/**
 * The main-column todo derivation (extracted so the `SidePane`'s tab
 * count badge and the panel's checklist consume the SAME derivation and
 * can never disagree):
 *
 * - Bridge todos (`todos_update`/`todos_clear` columns) win.
 * - `rawInput` fallback for non-bridge agents: when the bridge delivered
 *   no column, the latest `manage_todo_list` `rawInput` (from the ACP
 *   `tool_call` frame, via the `sessions` store) seeds the board. The
 *   `title` is the reliable discriminator (the `name` field is unstable in
 *   ACP 1.7).
 */
export function useMainTodoItems(sessionId: string | null): TodoItem[] {
  const column = useBridge((state) =>
    sessionId ? state.todos[sessionId] : undefined,
  );
  const messages = useSessions((state) =>
    sessionId ? state.messages[sessionId] : undefined,
  );

  // The latest `manage_todo_list` `rawInput` (written ops only). Derived
  // outside the selector (the selector returns stable references only, or
  // Zustand re-renders forever).
  const rawTodos = useMemo<TodoItem[] | undefined>(() => {
    if (!messages) return undefined;
    for (let i = messages.length - 1; i >= 0; i--) {
      const message = messages[i];
      if (
        message.kind !== "tool-call" ||
        message.title !== "manage_todo_list"
      ) {
        continue;
      }
      const raw = message.rawInput as
        { operation?: string; todoList?: unknown } | undefined;
      if (!raw || raw.operation === "read" || !Array.isArray(raw.todoList)) {
        continue;
      }
      const items = (raw.todoList as Array<Record<string, unknown>>)
        .filter(
          (t) => typeof t.content === "string" && typeof t.status === "string",
        )
        .map((t) => ({
          content: t.content as string,
          status: t.status as TodoItem["status"],
          description:
            typeof t.description === "string" ? t.description : undefined,
        }));
      if (items.length > 0) return items;
    }
    return undefined;
  }, [messages]);

  // Bridge todos win; the `rawInput` seeds the board only when the bridge
  // delivered no column for this session.
  return column?.main ?? rawTodos ?? [];
}

/**
 * The main column's OPEN (non-`completed`) todo count — the SHARED
 * derivation for the `SidePane`'s auto open/close (the pane's visibility
 * and the board's can never disagree — a fully-completed list counts as
 * ZERO: no board, and it does not keep the pane open either).
 */
export function useMainOpenTodoCount(sessionId: string | null): number {
  return useMainTodoItems(sessionId).filter(
    (t) => t.status !== "completed",
  ).length;
}

/** The three-state indicator: done = check circle, in-progress = `◉`, pending = `○`. */
function TodoIndicator({ status }: { status: TodoItem["status"] }) {
  if (status === "completed") {
    return (
      <span className="flex size-4 shrink-0 items-center justify-center rounded-full border border-success text-success">
        <Check className="size-3" />
      </span>
    );
  }
  if (status === "in_progress") {
    return <span aria-hidden className="text-ui-base text-warning">◉</span>;
  }
  return <span aria-hidden className="text-ui-base text-foreground-subtlest">○</span>;
}

function TodoItems({ items }: { items: TodoItem[] }) {
  return (
    <ol className="space-y-0.5">
      {items.map((item, i) => (
        <li
          key={i}
          className="flex h-8 items-center gap-2 rounded-md hover:bg-surface-hover"
        >
          <TodoIndicator status={item.status} />
          <span
            className={
              item.status === "completed"
                ? "text-ui-base text-foreground-subtle"
                : "text-ui-base text-foreground"
            }
          >
            {item.content}
          </span>
        </li>
      ))}
    </ol>
  );
}

/**
 * The session's todo board (the `SidePane`'s TODOS SECTION — the ZCode
 * `Goal`/`Progress` treatment: the header carries the label + the count,
 * the body the progress bar + checklist):
 *
 * - **Visible ONLY while open todos exist** (the user's rule): the board
 *   renders `null` when there is nothing open — no todos at all, or a
 *   fully-completed list (the old "No todos yet" placeholder is gone).
 *   The `SidePane`'s auto open/close shares the SAME `useMainOpenTodoCount`
 *   derivation, so the pane's visibility and the board can never
 *   disagree.
 * - Header: the `Todos` label + `N/M` + the `progress` bar (primary fill).
 *   checklist rows with the three-state indicators (done = a `size-4`
 *   check circle `text-success` + label `text-foreground-subtle`;
 *   in-progress = `◉` `text-warning`; pending = `○`
 *   `text-foreground-subtlest`) — the main section renders only when an
 *   OPEN main todo exists (a fully-completed main list is hidden even
 *   while a subagent column is still open).
 * - Subagent todo columns (`subagents[source]`, fed by the same bridge
 *   events — a unique `source` per child, cleared on child exit) as
 *   indented sub-rows (`pl-6`, `text-ui-sm`), one block per source with a
 *   `text-ui-xs text-foreground-subtlest` header — a column renders only
 *   while it has an OPEN item (an all-completed column is hidden).
 * - `rawInput` fallback for non-bridge agents (via `useMainTodoItems`).
 * - The `SidePane` frame owns the auto open/close (the board no longer
 *   auto-collapses on its own — the frame's auto open/close does it).
 */
export default function TodoBoardPanel({
  sessionId,
}: {
  sessionId: string | null;
}) {
  const mainItems = useMainTodoItems(sessionId);
  const subagents = useBridge((state) =>
    sessionId ? state.todos[sessionId]?.subagents : undefined,
  ) ?? {};
  // The visibility filter (the SAME open-item rule the badge's count uses
  // for the main column): a column renders only while it has an OPEN
  // item — a fully-completed column/list is hidden.
  const mainOpen = mainItems.filter((t) => t.status !== "completed").length;
  const subagentEntries = Object.entries(subagents).filter(([, items]) =>
    items.some((t) => t.status !== "completed"),
  );

  // Nothing open (no todos at all, or every todo completed) → hidden.
  if (mainOpen === 0 && subagentEntries.length === 0) {
    return null;
  }

  const completed = mainItems.filter((t) => t.status === "completed").length;

  return (
    <div className="flex flex-col gap-3">
      {mainOpen > 0 && (
        <div>
          {/* The section header (the ZCode `Goal` row treatment): the
              label + the open count (N/M — the completed count is part
              of the fraction; the OPEN count carries the cue). */}
          <div className="flex items-center gap-2">
            <p className="text-ui-base font-medium">Todos</p>
            <span className="text-ui-xs text-foreground-subtlest">
              {completed}/{mainItems.length}
            </span>
          </div>
          <Progress value={(completed / mainItems.length) * 100} />
        </div>
      )}
      {mainOpen > 0 && <TodoItems items={mainItems} />}
      {subagentEntries.map(([source, items]) => (
        <div key={source} className="pl-6">
          <p className="text-ui-xs text-foreground-subtlest">{source}</p>
          <div className="mt-0.5 space-y-0.5">
            {items.map((item, i) => (
              <div
                key={i}
                className="flex h-7 items-center gap-2 text-ui-sm"
              >
                <TodoIndicator status={item.status} />
                <span
                  className={
                    item.status === "completed"
                      ? "text-foreground-subtle"
                      : "text-foreground"
                  }
                >
                  {item.content}
                </span>
              </div>
            ))}
          </div>
        </div>
      ))}
    </div>
  );
}
