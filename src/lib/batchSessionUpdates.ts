import type { AcpSessionUpdate } from "./tauri";

export interface SessionUpdateBatchItem {
  sessionId: string;
  update: AcpSessionUpdate;
}

/**
 * Coalesce high-frequency `session-update` events into ONE store update per
 * animation frame.
 *
 * Why: a local agent at a high thinking level emits ~1000 chunks/sec. The
 * Rust driver forwards every notification as its own Tauri event, and each
 * event applied as its own zustand `setState` triggers its own React render
 * pass. ~1000 renders/sec saturates the webview main thread — the GPU sits
 * idle (it has no frames to composite) and the UI renders at ~1fps.
 *
 * Batching to one `setState` per `requestAnimationFrame` caps renders at the
 * display refresh rate (~60/sec) while keeping the transcript in step with
 * the stream (a frame's worth of chunks lands together, in order).
 *
 * `schedule` is injectable (the app passes `requestAnimationFrame`; tests
 * pass a manual queue) so the coalescing is verifiable without real frames.
 */
export function createBatchedSessionUpdate(
  apply: (items: SessionUpdateBatchItem[]) => void,
  schedule: (fn: () => void) => void,
): (item: SessionUpdateBatchItem) => void {
  let pending: SessionUpdateBatchItem[] = [];
  let scheduled = false;
  return (item) => {
    pending.push(item);
    if (!scheduled) {
      scheduled = true;
      schedule(() => {
        scheduled = false;
        const batch = pending;
        pending = [];
        apply(batch);
      });
    }
  };
}
