import { useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { Trash2Icon } from "lucide-react";
import { type FormEvent, useId, useRef, useState } from "react";
import { z } from "zod";
import { AgentAvatar, hideAgent, showAgent } from "@/entities/agent";
import { type Agent, api, unwrap, expectOk, upload } from "@/shared/api/client";
import { Agent as AgentSchema } from "@/shared/api/schemas";
import { m } from "@/shared/i18n/paraglide/messages";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/shared/ui/alert-dialog";
import { Button } from "@/shared/ui/button";
import { Input } from "@/shared/ui/input";
import { Label } from "@/shared/ui/label";
import { Textarea } from "@/shared/ui/textarea";

const Updated = z.object({ agent: AgentSchema });

export function AgentSettings({ agent }: { agent: Agent }) {
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const nameId = useId();
  const instructionsId = useId();
  const [name, setName] = useState(agent.name);
  const [instructions, setInstructions] = useState(agent.instructions);

  async function save(changes: { name?: string; instructions?: string }) {
    const { agent: saved } = await unwrap(
      api.PATCH("/agents/{id}", { params: { path: { id: agent.id } }, body: changes }),
    );
    showAgent(queryClient, saved);
  }

  async function remove() {
    await expectOk(api.DELETE("/agents/{id}", { params: { path: { id: agent.id } } }));
    hideAgent(queryClient, agent.id);
    await navigate({ to: "/", search: {} });
  }

  return (
    <div className="flex flex-col gap-6">
      <PhotoPicker agent={agent} onChange={(saved) => showAgent(queryClient, saved)} />
      <div className="flex flex-col gap-1.5">
        <Label htmlFor={nameId}>{m.name()}</Label>
        <Input
          id={nameId}
          value={name}
          maxLength={100}
          onChange={(event) => setName(event.target.value)}
          onBlur={() => {
            const trimmed = name.trim();
            if (trimmed && trimmed !== agent.name) void save({ name: trimmed });
            else setName(agent.name);
          }}
        />
      </div>
      <div className="flex flex-col gap-1.5">
        <Label htmlFor={instructionsId}>{m.instructions()}</Label>
        <Textarea
          id={instructionsId}
          value={instructions}
          maxLength={10_000}
          rows={6}
          onChange={(event) => setInstructions(event.target.value)}
          onBlur={() => {
            if (instructions !== agent.instructions) void save({ instructions });
          }}
        />
        <p className="text-xs text-muted-foreground">{m.saves_as_you_go()}</p>
      </div>
      <AlertDialog>
        <AlertDialogTrigger asChild>
          <Button variant="outline" className="self-start text-destructive hover:text-destructive">
            <Trash2Icon />
            {m.delete_agent()}
          </Button>
        </AlertDialogTrigger>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{m.delete_agent_title({ name: agent.name })}</AlertDialogTitle>
            <AlertDialogDescription>{m.delete_agent_body()}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{m.cancel()}</AlertDialogCancel>
            <AlertDialogAction variant="destructive" onClick={() => void remove()}>
              {m.delete()}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}

export function NewAgentForm({ onCancel }: { onCancel: () => void }) {
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const nameId = useId();
  const instructionsId = useId();
  const [name, setName] = useState("");
  const [instructions, setInstructions] = useState("");
  const [isCreating, setIsCreating] = useState(false);
  const [failed, setFailed] = useState(false);

  async function create(event: FormEvent) {
    event.preventDefault();
    setIsCreating(true);
    setFailed(false);
    try {
      const { agent } = await unwrap(
        api.POST("/agents", { body: { name: name.trim(), instructions } }),
      );
      showAgent(queryClient, agent);
      await navigate({ to: "/agents/$agentId", params: { agentId: agent.id }, search: {} });
    } catch {
      setFailed(true);
      setIsCreating(false);
    }
  }

  return (
    <form onSubmit={(event) => void create(event)} className="flex flex-col gap-6">
      <div className="flex flex-col gap-1.5">
        <Label htmlFor={nameId}>{m.name()}</Label>
        <Input
          id={nameId}
          value={name}
          required
          maxLength={100}
          autoFocus
          onChange={(event) => setName(event.target.value)}
        />
      </div>
      <div className="flex flex-col gap-1.5">
        <Label htmlFor={instructionsId}>
          {m.instructions()}{" "}
          <span className="font-normal text-muted-foreground">{m.optional()}</span>
        </Label>
        <Textarea
          id={instructionsId}
          value={instructions}
          maxLength={10_000}
          rows={5}
          placeholder={m.instructions_placeholder()}
          onChange={(event) => setInstructions(event.target.value)}
        />
      </div>
      {failed && (
        <p role="alert" className="text-sm text-destructive">
          {m.unreachable()}
        </p>
      )}
      <div className="flex justify-end gap-2">
        <Button type="button" variant="ghost" onClick={onCancel}>
          {m.cancel()}
        </Button>
        <Button type="submit" disabled={isCreating || !name.trim()}>
          {isCreating ? m.creating() : m.create()}
        </Button>
      </div>
    </form>
  );
}

function PhotoPicker({ agent, onChange }: { agent: Agent; onChange: (saved: Agent) => void }) {
  const input = useRef<HTMLInputElement>(null);
  const [failed, setFailed] = useState(false);

  async function choose(file: File | undefined) {
    if (!file) return;
    setFailed(false);
    const form = new FormData();
    form.append("image", file, file.name);
    try {
      const { agent: saved } = await upload("PUT", `/agents/${agent.id}/avatar`, form, Updated);
      onChange(saved);
    } catch {
      setFailed(true);
    }
  }

  async function removePhoto() {
    const { agent: saved } = await unwrap(
      api.DELETE("/agents/{id}/avatar", { params: { path: { id: agent.id } } }),
    );
    onChange(saved);
  }

  return (
    <div className="flex items-center gap-4">
      <AgentAvatar agent={agent} className="size-18 text-2xl" />
      <div className="flex flex-col gap-1">
        <div className="flex gap-2">
          <Button variant="outline" size="sm" onClick={() => input.current?.click()}>
            {agent.hasPhoto ? m.change_photo() : m.add_photo()}
          </Button>
          {agent.hasPhoto && (
            <Button variant="ghost" size="sm" onClick={() => void removePhoto()}>
              {m.remove()}
            </Button>
          )}
        </div>
        {failed && <p className="text-xs text-destructive">{m.photo_failed()}</p>}
      </div>
      <input
        ref={input}
        type="file"
        accept="image/jpeg,image/png"
        hidden
        onChange={(event) => void choose(event.target.files?.[0])}
      />
    </div>
  );
}
