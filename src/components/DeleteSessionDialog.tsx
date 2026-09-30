import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "./ui/dialog";
import { Button } from "./ui/button";

/**
 * Confirm dialog for deleting a session (the only DESTRUCTIVE action in
 * the sidebar — archive/unarchive are reversible and confirm-less). The
 * `SudoConfirmModal` structure: `ui/dialog` with `showCloseButton={false}`
 * (no X — the only exits are the footer's Cancel / Delete) + a
 * `destructive` Delete button.
 */
export default function DeleteSessionDialog({
  title,
  onConfirm,
  onClose,
}: {
  /** The session's title (as rendered on its row). */
  title: string;
  onConfirm: () => void;
  onClose: () => void;
}) {
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        // Escape / overlay click: the same exit as Cancel.
        if (!open) onClose();
      }}
    >
      <DialogContent showCloseButton={false} className="max-w-md">
        <DialogHeader>
          <DialogTitle>Delete session?</DialogTitle>
        </DialogHeader>
        <p className="truncate text-ui-base font-medium">{title}</p>
        <DialogDescription>
          This permanently removes the session's transcript from
          Archimedes' storage. This can't be undone.
        </DialogDescription>
        <DialogFooter>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="destructive" onClick={onConfirm}>
            Delete
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
