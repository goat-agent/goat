import { Link } from "@tanstack/react-router";
import { ChevronLeftIcon, MicIcon, MicOffIcon, PhoneOffIcon, Volume2Icon } from "lucide-react";
import { type ReactNode, useEffect, useState } from "react";
import { AgentAvatar } from "@/entities/agent";
import type { Agent } from "@/shared/api/client";
import { m } from "@/shared/i18n/paraglide/messages";
import { duration } from "@/shared/lib/time";
import { Button } from "@/shared/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/shared/ui/dropdown-menu";
import { type Call, canChooseSpeaker, chooseSpeaker, endCall, startCall, toggleMute } from "./call";

/** The conversation pane while a call is on: the face, what is happening, and the controls. */
export function CallScreen({
  agent,
  call,
  cards,
}: {
  agent: Agent;
  call: Call;
  /** Cards from messages during the call that need a tap, such as connecting a service. */
  cards?: ReactNode;
}) {
  const elapsed = useElapsed(call.since);
  const failed = call.phase === "failed";
  const status =
    call.phase === "connecting"
      ? m.call_connecting()
      : call.isMuted
        ? m.call_muted({ time: duration(elapsed) })
        : m.call_on({ time: duration(elapsed) });

  return (
    <section
      aria-label={m.call()}
      className="relative flex flex-1 flex-col items-center px-6 pt-5 pb-[max(1.75rem,env(safe-area-inset-bottom))]"
    >
      <Link
        to="/"
        aria-label={m.agents()}
        className="absolute top-2.5 left-2 grid size-11 place-items-center rounded-md text-muted-foreground md:hidden"
      >
        <ChevronLeftIcon className="size-5" />
      </Link>
      <div className="flex flex-1 flex-col items-center justify-center gap-2.5 text-center">
        <div className="relative mb-2.5 size-33">
          <AgentAvatar agent={agent} className="relative z-10 size-33 text-5xl" />
          <span
            aria-hidden
            className={`absolute -inset-2.5 rounded-full border-[3px] border-foreground/20 transition-opacity motion-safe:animate-pulse ${call.isSpeaking && !call.isMuted ? "opacity-100" : "opacity-0"}`}
          />
        </div>
        <h2 className="text-[22px] font-semibold">{agent.name}</h2>
        {failed ? (
          <div role="alert" className="flex max-w-80 flex-col items-center gap-1">
            <p className="font-semibold">
              {call.problem === "microphone" ? m.microphone_blocked() : m.call_failed()}
            </p>
            {call.problem === "microphone" && (
              <p className="text-[13px] text-muted-foreground">{m.microphone_blocked_hint()}</p>
            )}
            <Button
              variant="outline"
              className="mt-3"
              onClick={() => void startCall(call.agentId, call.after)}
            >
              {m.call_again()}
            </Button>
          </div>
        ) : (
          <p role="status" className="text-sm text-muted-foreground tabular-nums">
            {status}
          </p>
        )}
        {!failed && cards}
      </div>
      <div className="flex gap-7">
        {!failed && call.phase === "connected" && (
          <ControlButton
            label={m.mute()}
            pressed={call.isMuted}
            onClick={toggleMute}
            icon={call.isMuted ? <MicOffIcon /> : <MicIcon />}
          />
        )}
        {!failed && canChooseSpeaker && <SpeakerButton />}
        <ControlButton
          label={failed ? m.close() : m.end_call()}
          onClick={endCall}
          icon={<PhoneOffIcon />}
          tone="end"
        />
      </div>
    </section>
  );
}

function ControlButton({
  label,
  icon,
  onClick,
  pressed,
  tone,
}: {
  label: string;
  icon: ReactNode;
  onClick: () => void;
  pressed?: boolean;
  tone?: "end";
}) {
  const colors =
    tone === "end"
      ? "bg-red-600 text-white hover:bg-red-700"
      : pressed
        ? "bg-foreground text-background"
        : "bg-muted text-foreground hover:bg-accent";
  return (
    <button
      type="button"
      onClick={onClick}
      aria-pressed={pressed}
      className="flex flex-col items-center gap-2 text-xs text-muted-foreground"
    >
      <span className={`grid size-15 place-items-center rounded-full [&_svg]:size-5.5 ${colors}`}>
        {icon}
      </span>
      {label}
    </button>
  );
}

function SpeakerButton() {
  const [outputs, setOutputs] = useState<MediaDeviceInfo[]>([]);
  return (
    <DropdownMenu
      onOpenChange={(open) => {
        if (!open) return;
        void navigator.mediaDevices
          .enumerateDevices()
          .then((devices) => setOutputs(devices.filter(({ kind }) => kind === "audiooutput")));
      }}
    >
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          className="flex flex-col items-center gap-2 text-xs text-muted-foreground"
        >
          <span className="grid size-15 place-items-center rounded-full bg-muted text-foreground hover:bg-accent [&_svg]:size-5.5">
            <Volume2Icon />
          </span>
          {m.speaker()}
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="center">
        {outputs.map((output) => (
          <DropdownMenuItem
            key={output.deviceId}
            onSelect={() => void chooseSpeaker(output.deviceId)}
          >
            {output.label || m.speaker()}
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function useElapsed(since: number | undefined): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (since === undefined) return undefined;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [since]);
  return since === undefined ? 0 : Math.max(0, Math.floor((now - since) / 1000));
}
