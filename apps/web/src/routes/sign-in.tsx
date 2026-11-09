import { createFileRoute } from "@tanstack/react-router";
import { z } from "zod";
import { CodeForm, EmailForm } from "@/features/sign-in";
import { m } from "@/shared/i18n/paraglide/messages";
import { Logo } from "@/shared/ui/logo";

export const Route = createFileRoute("/sign-in")({
  validateSearch: z.object({ email: z.email().optional().catch(undefined) }),
  component: SignIn,
});

function SignIn() {
  const { email } = Route.useSearch();
  return (
    <main className="grid min-h-dvh place-items-center bg-background p-6">
      <div className="flex w-full max-w-90 flex-col gap-5">
        <div className="grid size-11 place-items-center rounded-xl bg-primary text-primary-foreground">
          <Logo className="size-7" />
        </div>
        <div>
          <h1 className="text-[22px] font-semibold">{email ? m.enter_code() : m.sign_in()}</h1>
          <p className="mt-1.5 text-sm text-muted-foreground">
            {email ? m.code_sent_to({ email }) : m.sign_in_hint()}
          </p>
        </div>
        {email ? <CodeForm email={email} /> : <EmailForm />}
      </div>
    </main>
  );
}
