import { useMemo } from "react";
import { useBridge, type TodoItem } from "../store/bridge";
import { useSessions } from "../store/sessions";

const STATUS_MARK: Record<TodoItem["status"], string> = {
  completed: "✓",
  in_progress: "◉",
  pending: "○",
};

function TodoItems({ items }: { items: TodoItem[] }) {
  return (
    <ol className="mt-1 space-y-1">
      {items.map((item, i) => (
        <li key={i} className="flex gap-2 text-xs">
          <span aria-hidden className="text-neutral-400">
            {STATUS_MARK[item.status]}
          </span>
          <span
            className={
              item.status === "completed"
                ? "text-neutral-600 line-through"
                : "text-neutral-300"
            }
          >
            {i + 1}. {item.content}
          </span>
        </li>
      ))}
    </ol>
  );
}

/**
 * Collapsible right rail: the session's todo board.
 *
 * - `main` column: numbered ✓/◉/○ items (completed = ✓, in_progress = ◉,
 *   pending = ○), from the `todos_update`/`todos_clear` bridge events.
 * - Subagent columns (`subagents[source]`): labeled sections, fed by the
 *   same bridge events (the existing `stream.ts` parent-bus relay — a
 *   unique `source` per child, cleared on child exit).
 * - `rawInput` fallback for non-bridge agents: when there are no bridge
 *   todos, the latest `manage_todo_list` `rawInput` (from the ACP
 *   `tool_call` frame, via the `sessions` store) seeds the board. The
 *   `title` is the reliable discriminator (the `name` field is unstable in
 *   ACP 1.7).
 * - Auto-collapses when empty (renders nothing).
 */
export default function TodoBoardPanel({
  sessionId,
}: {
  sessionId: string | null;
}) {
  const column = useBridge((state) =>
    sessionId ? state.todos[sessionId] : undefined,
  );
  const messages = useSessions((state) =>
    sessionId ? state.messages[sessionId] : undefined,
  );

  // The latest `manage_todo_list` `rawInput` (written ops only) — the
  // fallback for non-bridge agents. Derived outside the selector (the
  // selector returns stable references only, or Zustand re-renders forever).
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
  const mainItems = column?.main ?? rawTodos ?? [];
  const subagents = column?.subagents ?? {};
  const subagentEntries = Object.entries(subagents).filter(
    ([, items]) => items.length > 0,
  );

  // Auto-collapse when empty.
  if (mainItems.length === 0 && subagentEntries.length === 0) return null;

  const completed = mainItems.filter((t) => t.status === "completed").length;

  return (
    <aside className="w-64 shrink-0 overflow-y-auto border-l border-neutral-800 bg-neutral-950 p-3">
      <p className="text-xs font-medium text-neutral-400">
        Todo List — {completed}/{mainItems.length} completed
      </p>
      {mainItems.length > 0 && <TodoItems items={mainItems} />}
      {subagentEntries.length > 0 && (
        <div className="mt-3 space-y-3">
          {subagentEntries.map(([source, items]) => (
            <div key={source}>
              <p className="text-xs font-medium text-amber-400/70">{source}</p>
              <TodoItems items={items} />
            </div>
          ))}
        </div>
      )}
    </aside>
  );
}
