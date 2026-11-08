import { useQueryClient } from "@tanstack/react-query";
import { Link, Navigate } from "@tanstack/react-router";
import {
  ChevronDownIcon,
  ChevronLeftIcon,
  ClockIcon,
  PhoneIcon,
  TriangleAlertIcon,
} from "lucide-react";
import { useEffect, useRef } from "react";
import { CallScreen, startCall, useCall } from "@/features/call";
import { ConnectCard } from "@/features/connect-integration";
import {
  type Pending,
  prunePending,
  stillPending,
  useComposer,
  usePending,
} from "@/features/send-message";
import { AgentAvatar, isWorking, useAgents } from "@/entities/agent";
import { useRecheckSession } from "@/entities/me";
import {
  AgentTurn,
  allMessages,
  DayDivider,
  newestMessageId,
  TypingBubble,
  useConversation,
  UserTurn,
} from "@/entities/message";
import type { Agent, Message } from "@/shared/api/client";
import { m } from "@/shared/i18n/paraglide/messages";
import { isSameDay } from "@/shared/lib/time";
import { useWindowClass } from "@/shared/lib/window-class";
import {
  MessageScroller,
  MessageScrollerButton,
  MessageScrollerContent,
  MessageScrollerItem,
  MessageScrollerProvider,
  MessageScrollerViewport,
} from "@/shared/ui/message-scroller";
import { SidebarTrigger } from "@/shared/ui/sidebar";
import { Composer } from "./composer";

export function ConversationPane({ agentId }: { agentId: string }) {
  const agents = useAgents();
  const call = useCall();
  if (!agents) return <div className="flex-1" />;
  const agent = agents.find(({ id }) => id === agentId);
  // A link to an agent that was deleted, or that belongs to someone else.
  if (!agent) return <Navigate to="/" search={{}} replace />;
  if (call?.agentId === agentId) return <CallPane agent={agent} />;
  return <ChatPane key={agentId} agent={agent} />;
}

function ChatPane({ agent }: { agent: Agent }) {
  const conversation = useConversation(agent.id, useRecheckSession());
  const { messages } = conversation;
  const composer = useComposer(agent.id);
  const pending = stillPending(usePending(agent.id), messages);
  const call = useCall();
  const { data } = conversation;
  useEffect(() => {
    prunePending(agent.id, allMessages(data));
  }, [agent.id, data]);

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <Header agent={agent} />
      {conversation.isSuccess ? (
        // Mounted once the newest page is in, so it opens at the end instead of the top.
        <MessageScrollerProvider autoScroll defaultScrollPosition="end">
          <MessageScroller className="min-h-0 flex-1">
            <MessageScrollerViewport preserveScrollOnPrepend aria-label={m.messages()}>
              <MessageScrollerContent className="mx-auto w-full max-w-[45rem] gap-[18px] px-4 pt-6 pb-4 md:px-6">
                <OlderMessages
                  hasMore={conversation.hasPreviousPage}
                  isLoading={conversation.isFetchingPreviousPage}
                  load={() => void conversation.fetchPreviousPage()}
                />
                {messages.length === 0 && pending.length === 0 && <Greeting agent={agent} />}
                <Turns agent={agent} messages={messages} />
                {pending.map((item) => (
                  <MessageScrollerItem key={item.localId} messageId={item.localId} scrollAnchor>
                    <PendingTurn agent={agent} pending={item} onResend={composer.resend} />
                  </MessageScrollerItem>
                ))}
                {isWorking(agent) && (
                  <MessageScrollerItem messageId="typing">
                    <TypingBubble />
                  </MessageScrollerItem>
                )}
              </MessageScrollerContent>
            </MessageScrollerViewport>
            <MessageScrollerButton />
          </MessageScroller>
        </MessageScrollerProvider>
      ) : (
        <div className="flex-1" />
      )}
      <Composer
        agentId={agent.id}
        agentName={agent.name}
        composer={composer}
        isMicrophoneBusy={call !== undefined}
      />
    </div>
  );
}

function Turns({ agent, messages }: { agent: Agent; messages: Message[] }) {
  return messages.map((message, index) => {
    const date = new Date(message.createdAt);
    const previous = messages[index - 1];
    const newDay = !previous || !isSameDay(new Date(previous.createdAt), date);
    return (
      <MessageScrollerItem
        key={message.id}
        messageId={String(message.id)}
        scrollAnchor={message.role === "user"}
        className="flex flex-col gap-[18px]"
      >
        {newDay && <DayDivider date={date} />}
        {message.role === "user" ? (
          <UserTurn agentId={agent.id} message={message} />
        ) : (
          <AgentTurn message={message}>
            {message.connect?.map((request) => (
              <ConnectCard key={request.integrationId} request={request} agentId={agent.id} />
            ))}
          </AgentTurn>
        )}
      </MessageScrollerItem>
    );
  });
}

function Greeting({ agent }: { agent: Agent }) {
  return (
    <div className="my-auto flex flex-col items-center gap-3.5 py-10 text-center">
      <AgentAvatar agent={agent} className="size-18 text-3xl" />
      <p className="text-xl font-semibold">{m.greeting({ name: agent.name })}</p>
      <p className="text-sm text-muted-foreground">{m.greeting_ask()}</p>
    </div>
  );
}

function PendingTurn({
  agent,
  pending,
  onResend,
}: {
  agent: Agent;
  pending: Pending;
  onResend: (pending: Pending) => void;
}) {
  const footer =
    pending.state === "failed" ? (
      <p role="alert" className="flex items-center gap-1 text-xs text-destructive">
        <TriangleAlertIcon className="size-3" />
        {m.not_sent()}
        <button
          type="button"
          onClick={() => onResend(pending)}
          className="font-medium underline underline-offset-2"
        >
          {m.send_again()}
        </button>
      </p>
    ) : pending.state === "sent" && isWorking(agent) ? (
      <p className="flex items-center gap-1 text-xs text-muted-foreground">
        <ClockIcon className="size-3" />
        {m.after_this_task()}
      </p>
    ) : undefined;
  return <UserTurn agentId={agent.id} message={pending} footer={footer} />;
}

/** Loads the page before the oldest loaded one when the reader reaches the top. */
function OlderMessages({
  hasMore,
  isLoading,
  load,
}: {
  hasMore: boolean;
  isLoading: boolean;
  load: () => void;
}) {
  const sentinel = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const element = sentinel.current;
    if (!element || !hasMore) return undefined;
    const observer = new IntersectionObserver(([entry]) => {
      if (entry?.isIntersecting && !isLoading) load();
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, [hasMore, isLoading, load]);
  return hasMore ? <div ref={sentinel} className="h-px" aria-hidden /> : null;
}

function Header({ agent }: { agent: Agent }) {
  const windowClass = useWindowClass();
  const queryClient = useQueryClient();

  function call() {
    void startCall(agent.id, newestMessageId(queryClient, agent.id));
  }

  return (
    <header className="flex h-13 shrink-0 items-center gap-1 border-b px-2">
      {windowClass === "compact" ? (
        <Link
          to="/"
          aria-label={m.agents()}
          className="grid size-11 place-items-center rounded-md text-muted-foreground hover:bg-muted"
        >
          <ChevronLeftIcon className="size-5" />
        </Link>
      ) : windowClass === "expanded" ? (
        <SidebarTrigger className="size-9 text-muted-foreground" />
      ) : (
        <span className="size-9" />
      )}
      <span className="flex-1" />
      <Link
        to="."
        search={(previous) => ({ ...previous, dialog: "agent" as const })}
        className="flex min-h-9 items-center gap-2 rounded-full py-1 ps-1 pe-2.5 hover:bg-muted"
      >
        <AgentAvatar agent={agent} className="size-7 text-xs" />
        <span className="font-semibold">{agent.name}</span>
        <ChevronDownIcon className="size-3.5 text-muted-foreground" />
      </Link>
      <span className="flex-1" />
      <button
        type="button"
        onClick={call}
        aria-label={m.call()}
        className="grid size-9 place-items-center rounded-md text-muted-foreground hover:bg-muted hover:text-foreground pointer-coarse:size-11"
      >
        <PhoneIcon className="size-4" />
      </button>
    </header>
  );
}

function CallPane({ agent }: { agent: Agent }) {
  const call = useCall();
  const { messages } = useConversation(agent.id, useRecheckSession());
  if (!call) return null;
  const requests = messages
    .filter((message) => message.id > call.after)
    .flatMap((message) => message.connect ?? []);
  return (
    <CallScreen
      agent={agent}
      call={call}
      cards={
        requests.length > 0 && (
          <div className="mt-4 flex flex-col items-center gap-2">
            {requests.map((request) => (
              <ConnectCard key={request.integrationId} request={request} agentId={agent.id} />
            ))}
          </div>
        )
      }
    />
  );
}
