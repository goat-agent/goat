import { ChevronDownIcon, FileIcon, LaptopIcon } from "lucide-react";
import type { ReactNode } from "react";
import { fileUrl, type Message, type SentFile } from "@/shared/api/client";
import { m } from "@/shared/i18n/paraglide/messages";
import { dayLabel } from "@/shared/lib/time";
import { Bubble, BubbleContent } from "@/shared/ui/bubble";
import { Dots } from "@/shared/ui/dots";
import { Markdown } from "@/shared/ui/markdown";

// A turn is at most 34rem wide so long replies keep a readable line length.
const TURN = "flex max-w-[min(85%,34rem)] flex-col gap-1";
const BUBBLE = "rounded-[20px] px-4 py-2.5 text-[15px] leading-relaxed";

export function UserTurn({
  agentId,
  message,
  footer,
}: {
  agentId: string;
  message: Pick<Message, "text" | "files">;
  footer?: ReactNode;
}) {
  return (
    <div className={`${TURN} items-end self-end`}>
      {message.files?.map((file) => (
        <SentFileView key={file.id} agentId={agentId} file={file} />
      ))}
      {message.text && (
        <Bubble variant="default" align="end" className="max-w-full">
          <BubbleContent className={`${BUBBLE} whitespace-pre-wrap`}>{message.text}</BubbleContent>
        </Bubble>
      )}
      {footer}
    </div>
  );
}

export function AgentTurn({ message, children }: { message: Message; children?: ReactNode }) {
  return (
    <div className={`${TURN} items-start`}>
      {message.steps && message.steps.length > 0 && <Steps steps={message.steps} />}
      {message.text && (
        <Bubble variant="muted" className="max-w-full">
          <BubbleContent className={BUBBLE}>
            <Markdown text={message.text} />
          </BubbleContent>
        </Bubble>
      )}
      {children}
    </div>
  );
}

/** What the agent did on the user's computer: one quiet line that opens into the list. */
function Steps({ steps }: { steps: string[] }) {
  return (
    <details className="group">
      <summary className="inline-flex min-h-6 cursor-pointer list-none items-center gap-1.5 rounded-md px-1 text-xs text-muted-foreground hover:text-foreground [&::-webkit-details-marker]:hidden">
        <LaptopIcon className="size-3.5" />
        {m.steps_summary({ count: steps.length })}
        <ChevronDownIcon className="size-3 transition-transform group-open:rotate-180" />
      </summary>
      <ol className="flex flex-col gap-0.5 py-1 ps-6 text-[13px] text-muted-foreground">
        {steps.map((step, index) => (
          // Steps have no id and never reorder, so their position names them.
          <li key={`${index}:${step}`}>{step}</li>
        ))}
      </ol>
    </details>
  );
}

export function SentFileView({ agentId, file }: { agentId: string; file: SentFile }) {
  const url = fileUrl(agentId, file.id);
  if (file.mediaType.startsWith("image/")) {
    return (
      <a href={url} target="_blank" rel="noopener noreferrer" className="block">
        <img
          src={url}
          alt={file.name}
          className="h-auto max-h-72 w-60 rounded-2xl object-cover ring-1 ring-border"
        />
      </a>
    );
  }
  return (
    <a
      href={url}
      target="_blank"
      rel="noopener noreferrer"
      className="flex items-center gap-2.5 rounded-2xl border bg-background py-2 ps-2 pe-3.5 hover:bg-muted"
    >
      <span className="grid size-9 place-items-center rounded-[10px] bg-muted text-muted-foreground">
        <FileIcon className="size-4" />
      </span>
      <span className="max-w-48 truncate text-[13px] font-medium">{file.name}</span>
    </a>
  );
}

export function TypingBubble() {
  return (
    <div
      role="status"
      aria-label={m.writing_reply()}
      className="inline-flex h-10 items-center gap-1 self-start rounded-[20px] bg-muted px-4 text-muted-foreground"
    >
      <Dots />
    </div>
  );
}

export function DayDivider({ date }: { date: Date }) {
  return <div className="self-center text-xs text-muted-foreground">{dayLabel(date)}</div>;
}
