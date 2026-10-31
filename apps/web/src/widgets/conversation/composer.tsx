import { ArrowUpIcon, MicIcon, PlusIcon, SquareIcon, Trash2Icon } from "lucide-react";
import { type ClipboardEvent, type DragEvent, type KeyboardEvent, useRef } from "react";
import { useDictation } from "@/features/dictate";
import { AttachmentChips, type ComposerState } from "@/features/send-message";
import { m } from "@/shared/i18n/paraglide/messages";
import { duration } from "@/shared/lib/time";

/** One block for writing: files, text, attach, dictate, and send. */
export function Composer({
  agentId,
  agentName,
  composer,
  isMicrophoneBusy,
}: {
  agentId: string;
  agentName: string;
  composer: ComposerState;
  /** A call holds the microphone, so dictation waits. */
  isMicrophoneBusy: boolean;
}) {
  const input = useRef<HTMLInputElement>(null);
  const text = useRef<HTMLTextAreaElement>(null);
  const dictation = useDictation(agentId, (said) => {
    composer.setText(composer.text ? `${composer.text} ${said}` : said);
    text.current?.focus();
  });

  function keyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    // Enter that confirms a Korean syllable must not send the message.
    if (event.key !== "Enter" || event.shiftKey || event.nativeEvent.isComposing) return;
    event.preventDefault();
    composer.send();
  }

  function paste(event: ClipboardEvent) {
    if (event.clipboardData.files.length === 0) return;
    event.preventDefault();
    composer.attach(event.clipboardData.files);
  }

  function drop(event: DragEvent) {
    if (event.dataTransfer.files.length === 0) return;
    event.preventDefault();
    composer.attach(event.dataTransfer.files);
  }

  const problem =
    dictation.problem === "microphone"
      ? m.microphone_blocked()
      : dictation.problem === "transcription"
        ? m.transcription_failed()
        : undefined;

  return (
    <div className="px-2.5 pb-[max(0.625rem,env(safe-area-inset-bottom))] md:px-6 md:pb-5">
      <div
        onDragOver={(event) => event.preventDefault()}
        onDrop={drop}
        className="mx-auto flex max-w-[42rem] flex-col gap-1 rounded-3xl border bg-background p-2 shadow-sm focus-within:border-ring focus-within:ring-3 focus-within:ring-ring/25"
      >
        {problem && (
          <p role="alert" className="px-2.5 pt-1 text-xs text-destructive">
            {problem}
          </p>
        )}
        <AttachmentChips
          attachments={composer.attachments}
          onRetry={composer.retry}
          onRemove={composer.remove}
        />
        {dictation.state === "idle" ? (
          <>
            <textarea
              ref={text}
              rows={1}
              value={composer.text}
              onChange={(event) => composer.setText(event.target.value)}
              onKeyDown={keyDown}
              onPaste={paste}
              placeholder={m.message_to({ name: agentName })}
              aria-label={m.message()}
              className="max-h-60 min-h-9 resize-none bg-transparent px-2.5 py-1.5 text-[15px] outline-none [field-sizing:content] placeholder:text-muted-foreground"
            />
            <div className="flex items-center gap-1">
              <RoundButton
                label={m.attach()}
                onClick={() => input.current?.click()}
                disabled={!composer.canAttach}
              >
                <PlusIcon className="size-[18px]" />
              </RoundButton>
              <span className="flex-1" />
              <RoundButton
                label={m.dictate()}
                onClick={() => void dictation.start()}
                disabled={isMicrophoneBusy}
              >
                <MicIcon className="size-[18px]" />
              </RoundButton>
              <button
                type="button"
                onClick={composer.send}
                aria-label={m.send()}
                aria-disabled={!composer.canSend}
                className="grid size-9 place-items-center rounded-full bg-primary text-primary-foreground aria-disabled:opacity-35 pointer-coarse:size-11"
              >
                <ArrowUpIcon className="size-4" />
              </button>
            </div>
          </>
        ) : (
          <div className="flex items-center gap-1">
            <RoundButton
              label={m.discard_recording()}
              onClick={dictation.discard}
              disabled={dictation.state === "transcribing"}
            >
              <Trash2Icon className="size-[18px]" />
            </RoundButton>
            <div
              role="status"
              className="flex flex-1 items-center gap-2.5 px-1.5 text-[13px] text-muted-foreground tabular-nums"
            >
              {dictation.state === "recording" ? (
                <>
                  <span className="size-2 rounded-full bg-red-600 motion-safe:animate-pulse" />
                  {m.recording({ time: duration(dictation.seconds) })}
                </>
              ) : (
                m.transcribing()
              )}
            </div>
            <button
              type="button"
              onClick={dictation.stop}
              disabled={dictation.state === "transcribing"}
              aria-label={m.stop_recording()}
              className="grid size-9 place-items-center rounded-full bg-primary text-primary-foreground disabled:opacity-35 pointer-coarse:size-11"
            >
              <SquareIcon className="size-3.5 fill-current" />
            </button>
          </div>
        )}
      </div>
      <input
        ref={input}
        type="file"
        multiple
        hidden
        onChange={(event) => {
          if (event.target.files) composer.attach(event.target.files);
          event.target.value = "";
        }}
      />
    </div>
  );
}

function RoundButton({
  label,
  onClick,
  disabled,
  children,
}: {
  label: string;
  onClick: () => void;
  disabled?: boolean;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      aria-label={label}
      className="grid size-9 place-items-center rounded-full text-muted-foreground hover:bg-muted hover:text-foreground disabled:opacity-40 pointer-coarse:size-11"
    >
      {children}
    </button>
  );
}
