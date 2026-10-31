import { useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { LogOutIcon } from "lucide-react";
import { useId, useState } from "react";
import { meQuery, useMe } from "@/entities/me";
import { api, expectOk } from "@/shared/api/client";
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

/** `beforeSignOut` lets the caller stop what must not outlive the session, such as a call. */
export function AccountForm({ beforeSignOut }: { beforeSignOut: () => void }) {
  const me = useMe();
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const nameId = useId();
  const [name, setName] = useState(me.name);

  async function rename() {
    const trimmed = name.trim();
    if (trimmed === me.name) return;
    await expectOk(api.PATCH("/me", { body: { name: trimmed } }));
    await queryClient.invalidateQueries({ queryKey: meQuery.queryKey });
  }

  async function signOut() {
    beforeSignOut();
    await expectOk(api.POST("/auth/sign-out"));
    queryClient.clear();
    await navigate({ to: "/sign-in", search: {} });
  }

  return (
    <div className="flex flex-col gap-6">
      <div className="flex flex-col gap-1.5">
        <Label htmlFor={nameId}>{m.name()}</Label>
        <Input
          id={nameId}
          value={name}
          maxLength={100}
          autoComplete="name"
          onChange={(event) => setName(event.target.value)}
          onBlur={() => void rename()}
        />
        <p className="text-xs text-muted-foreground">{m.name_hint()}</p>
      </div>
      <div className="flex flex-col gap-1.5">
        <Label>{m.email()}</Label>
        <p className="flex h-9 items-center rounded-md bg-muted px-3 text-sm text-muted-foreground">
          {me.email}
        </p>
      </div>
      <AlertDialog>
        <AlertDialogTrigger asChild>
          <Button variant="outline" className="self-start">
            <LogOutIcon />
            {m.sign_out()}
          </Button>
        </AlertDialogTrigger>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{m.sign_out_title()}</AlertDialogTitle>
            <AlertDialogDescription>{m.sign_out_body()}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{m.cancel()}</AlertDialogCancel>
            <AlertDialogAction onClick={() => void signOut()}>{m.sign_out()}</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
