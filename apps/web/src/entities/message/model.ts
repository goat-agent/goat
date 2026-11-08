import type { InfiniteData } from "@tanstack/react-query";
import type { Message } from "@/shared/api/client";

export type MessagePage = { messages: Message[] };

export type Conversation = InfiniteData<MessagePage, number | undefined>;

export function withMessage(
  conversation: Conversation | undefined,
  message: Message,
): Conversation | undefined {
  if (!conversation) return conversation;
  const pages = conversation.pages.map((page) => ({
    messages: page.messages.filter(({ id }) => id !== message.id),
  }));
  const last = pages.at(-1);
  if (!last) return { ...conversation, pages: [{ messages: [message] }] };

  const index = last.messages.findIndex(({ id }) => id > message.id);
  last.messages.splice(index === -1 ? last.messages.length : index, 0, message);
  return { ...conversation, pages };
}

export function allMessages(conversation: Conversation | undefined): Message[] {
  return conversation?.pages.flatMap((page) => page.messages) ?? [];
}
