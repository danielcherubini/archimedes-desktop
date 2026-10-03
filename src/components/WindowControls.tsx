import { getCurrentWindow } from "@tauri-apps/api/window";
import { MinusIcon, SquareIcon, XIcon } from "lucide-react";

/**
 * Frameless window controls. The window is created `decorations: false`
 * (see tauri.conf.json), so the native titlebar — and its close / minimize /
 * maximize buttons — is gone. These replace it. The surrounding top bar in
 * App.tsx is the drag region (`data-tauri-drag-region="deep"` on the
 * container — "deep" extends the region to the whole subtree, since a bare
 * attribute only drags from the element clicked DIRECTLY): the bar's empty
 * areas drag the window (double-click maximizes) while these buttons
 * (interactive elements without the attribute) still block it and stay
 * clickable.
 */
export default function WindowControls() {
  const win = getCurrentWindow();

  return (
    <div className="flex items-center gap-0.5">
      <button
        type="button"
        title="Minimize"
        onClick={() => void win.minimize()}
        className="flex h-7 w-7 items-center justify-center rounded-md text-foreground-subtle transition-colors hover:bg-hover hover:text-foreground"
      >
        <MinusIcon size={14} aria-hidden />
      </button>
      <button
        type="button"
        title="Maximize / Restore"
        onClick={() => void win.toggleMaximize()}
        className="flex h-7 w-7 items-center justify-center rounded-md text-foreground-subtle transition-colors hover:bg-hover hover:text-foreground"
      >
        <SquareIcon size={12} aria-hidden />
      </button>
      <button
        type="button"
        title="Close"
        onClick={() => void win.close()}
        className="flex h-7 w-7 items-center justify-center rounded-md text-foreground-subtle transition-colors hover:bg-destructive/20 hover:text-destructive"
      >
        <XIcon size={14} aria-hidden />
      </button>
    </div>
  );
}
