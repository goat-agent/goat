import { FileIcon, Loader2Icon, RotateCcwIcon, XIcon } from "lucide-react";
import { m } from "@/shared/i18n/paraglide/messages";
import type { Attachment } from "./composer";

export function AttachmentChips({
  attachments,
  onRetry,
  onRemove,
}: {
  attachments: Attachment[];
  onRetry: (localId: string) => void;
  onRemove: (localId: string) => void;
}) {
  if (attachments.length === 0) return null;
  return (
    <ul className="flex flex-wrap gap-2 px-0.5 pt-0.5 pb-1">
      {attachments.map(({ localId, file, state }) => {
        const failed = state.kind === "failed";
        return (
          <li
            key={localId}
            className={`flex max-w-60 items-center gap-2 rounded-[14px] bg-muted p-1.5 text-xs ${failed ? "text-destructive" : ""}`}
          >
            {failed ? (
              <button
                type="button"
                onClick={() => onRetry(localId)}
                aria-label={m.upload_again({ name: file.name })}
                className="grid size-8 shrink-0 place-items-center rounded-[10px] bg-background"
              >
                <RotateCcwIcon className="size-3.5" />
              </button>
            ) : (
              <span className="grid size-8 shrink-0 place-items-center rounded-[10px] bg-background text-muted-foreground">
                {state.kind === "uploading" ? (
                  <Loader2Icon className="size-3.5 animate-spin" aria-label={m.uploading()} />
                ) : (
                  <FileIcon className="size-3.5" />
                )}
              </span>
            )}
            <span className="min-w-0">
              <span className="block truncate font-medium">{file.name}</span>
              {failed && <span className="block">{m.not_uploaded()}</span>}
            </span>
            <button
              type="button"
              onClick={() => onRemove(localId)}
              aria-label={m.remove_file({ name: file.name })}
              className="grid size-6 shrink-0 place-items-center rounded-full text-muted-foreground hover:bg-background"
            >
              <XIcon className="size-3" />
            </button>
          </li>
        );
      })}
    </ul>
  );
}
