import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { z } from "zod";
import { newestMessageId } from "@/entities/message";
import { api, expectOk, type SentFile, upload } from "@/shared/api/client";
import { SentFile as SentFileSchema } from "@/shared/api/schemas";
import { addPending, type Pending, setPendingState } from "./pending";

const MAX_ATTACHMENTS = 10;

export type Attachment = {
  localId: string;
  file: File;
  state: { kind: "uploading" } | { kind: "uploaded"; sent: SentFile } | { kind: "failed" };
};

const Uploaded = z.object({ file: SentFileSchema });

export function useComposer(agentId: string) {
  const queryClient = useQueryClient();
  const [text, setText] = useState("");
  const [attachments, setAttachments] = useState<Attachment[]>([]);

  function setState(localId: string, state: Attachment["state"]) {
    setAttachments((list) =>
      list.map((item) => (item.localId === localId ? { ...item, state } : item)),
    );
  }

  async function uploadOne(localId: string, file: File) {
    setState(localId, { kind: "uploading" });
    const form = new FormData();
    form.append("file", file, file.name);
    try {
      const { file: sent } = await upload("POST", `/agents/${agentId}/files`, form, Uploaded);
      setState(localId, { kind: "uploaded", sent });
    } catch {
      setState(localId, { kind: "failed" });
    }
  }

  function attach(files: Iterable<File>) {
    const room = MAX_ATTACHMENTS - attachments.length;
    const added = Array.from(files)
      .slice(0, Math.max(0, room))
      .map((file) => ({
        localId: crypto.randomUUID(),
        file,
        state: { kind: "uploading" } as const,
      }));
    setAttachments((list) => [...list, ...added]);
    for (const item of added) void uploadOne(item.localId, item.file);
  }

  function retry(localId: string) {
    const item = attachments.find((attachment) => attachment.localId === localId);
    if (item) void uploadOne(localId, item.file);
  }

  function remove(localId: string) {
    setAttachments((list) => list.filter((item) => item.localId !== localId));
  }

  const uploaded = attachments.flatMap((item) =>
    item.state.kind === "uploaded" ? [item.state.sent] : [],
  );
  const isUploading = attachments.some((item) => item.state.kind === "uploading");
  const canSend = !isUploading && (text.trim().length > 0 || uploaded.length > 0);

  async function deliver(pending: Pending) {
    try {
      await expectOk(
        api.POST("/agents/{id}/messages", {
          params: { path: { id: agentId } },
          body: { text: pending.text, files: pending.files.map(({ id }) => id) },
        }),
      );
      setPendingState(agentId, pending.localId, "sent");
    } catch {
      setPendingState(agentId, pending.localId, "failed");
    }
  }

  function send() {
    if (!canSend) return;
    const pending: Pending = {
      localId: crypto.randomUUID(),
      text: text.trim(),
      files: uploaded,
      after: newestMessageId(queryClient, agentId),
      state: "sending",
    };
    addPending(agentId, pending);
    setText("");
    setAttachments([]);
    void deliver(pending);
  }

  function resend(pending: Pending) {
    setPendingState(agentId, pending.localId, "sending");
    void deliver(pending);
  }

  return {
    text,
    setText,
    attachments,
    attach,
    retry,
    remove,
    canAttach: attachments.length < MAX_ATTACHMENTS,
    canSend,
    send,
    resend,
  };
}

export type ComposerState = ReturnType<typeof useComposer>;
