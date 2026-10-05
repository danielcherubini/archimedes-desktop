import { X } from "lucide-react";
import type { ChatComposerAttachment } from "../../lib/chatAttachments";

/**
 * The composer's staged-image thumbnail strip.
 *
 * Props-only: the `attachments` state + the object-URL lifecycle (staging,
 * release-on-remove, release-on-session-switch, release-on-unmount) all stay
 * in `ChatStream` — they coordinate with `send()` and the session-switch
 * effect. Extracted verbatim from `ChatStream` (identical DOM); renders
 * nothing while nothing is staged.
 */
export default function AttachmentStrip({
  attachments,
  onRemove,
}: {
  attachments: ChatComposerAttachment[];
  onRemove: (id: string) => void;
}) {
  return (
    <>
      {attachments.length > 0 && (
        <div className="mb-2 flex flex-wrap gap-2">
          {attachments.map((att) => (
            <div
              key={att.id}
              className="group relative size-12 overflow-hidden rounded-lg border border-input-border bg-input"
            >
              <img
                src={att.objectUrl}
                alt={att.filename}
                className="size-full object-cover"
              />
              <button
                type="button"
                aria-label="Remove image attachment"
                onClick={() => onRemove(att.id)}
                className="absolute right-0.5 top-0.5 size-4 rounded-full bg-input p-0 opacity-0 transition-opacity group-hover:opacity-100"
              >
                <X className="size-3 text-foreground" />
              </button>
            </div>
          ))}
        </div>
      )}
    </>
  );
}
