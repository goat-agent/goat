import { createFileRoute, Link, useRouter } from "@tanstack/react-router";
import { useEffect, useRef, useState } from "react";
import { z } from "zod";
import { finishConnecting, takeReturnPath } from "@/features/connect-integration";
import { m } from "@/shared/i18n/paraglide/messages";

export const Route = createFileRoute("/connected")({
  validateSearch: z.object({
    integration: z.string().optional().catch(undefined),
    state: z.string().optional().catch(undefined),
    code: z.string().optional().catch(undefined),
    error: z.string().optional().catch(undefined),
  }),
  component: Connected,
});

function Connected() {
  const { integration, state, code, error } = Route.useSearch();
  const router = useRouter();
  const [returnPath] = useState(takeReturnPath);
  const [failed, setFailed] = useState(false);
  // The state in the address works once; StrictMode's second effect run must not spend it again.
  const started = useRef(false);
  const usable = error === undefined && integration && state && code;

  useEffect(() => {
    if (!usable || started.current) return;
    started.current = true;
    finishConnecting(integration, state, code).then(
      () => router.history.replace(returnPath),
      () => setFailed(true),
    );
  }, [usable, integration, state, code, returnPath, router]);

  return (
    <main className="grid min-h-dvh place-items-center p-6 text-center">
      {usable && !failed ? (
        <p role="status" className="text-sm text-muted-foreground">
          {m.connecting()}
        </p>
      ) : (
        <div className="flex flex-col items-center gap-3">
          <p className="font-medium">
            {error === "denied" ? m.connect_denied() : m.connect_failed()}
          </p>
          <Link
            to={returnPath}
            className="text-sm text-muted-foreground underline underline-offset-2"
          >
            {m.go_back()}
          </Link>
        </div>
      )}
    </main>
  );
}
