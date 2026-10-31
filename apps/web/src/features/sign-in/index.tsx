import { useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { type FormEvent, useEffect, useState } from "react";
import { api, ApiError, expectOk } from "@/shared/api/client";
import { m } from "@/shared/i18n/paraglide/messages";
import { browserDevice } from "@/shared/lib/browser";
import { Button } from "@/shared/ui/button";
import { Input } from "@/shared/ui/input";
import { InputOTP, InputOTPGroup, InputOTPSlot } from "@/shared/ui/input-otp";
import { Label } from "@/shared/ui/label";

const RESEND_SECONDS = 30;

function sendCode(email: string): Promise<void> {
  return expectOk(api.POST("/auth/email-code", { body: { email } }));
}

export function EmailForm() {
  const navigate = useNavigate();
  const [email, setEmail] = useState("");
  const [isSending, setIsSending] = useState(false);
  const [error, setError] = useState<string>();

  async function submit(event: FormEvent) {
    event.preventDefault();
    setIsSending(true);
    setError(undefined);
    try {
      await sendCode(email.trim());
      await navigate({ to: "/sign-in", search: { email: email.trim() } });
    } catch (cause) {
      setError(
        cause instanceof ApiError && cause.status === 400 ? m.check_email() : m.unreachable(),
      );
    } finally {
      setIsSending(false);
    }
  }

  return (
    <form onSubmit={(event) => void submit(event)} className="flex flex-col gap-4">
      <div className="flex flex-col gap-2">
        <Label htmlFor="email">{m.email()}</Label>
        <Input
          id="email"
          name="email"
          type="email"
          inputMode="email"
          autoComplete="email"
          required
          autoFocus
          value={email}
          onChange={(event) => setEmail(event.target.value)}
          aria-invalid={error !== undefined}
          aria-describedby={error ? "email-error" : undefined}
          className="h-11"
        />
        {error && (
          <p id="email-error" role="alert" className="text-sm text-destructive">
            {error}
          </p>
        )}
      </div>
      <Button type="submit" size="lg" className="h-11" disabled={isSending}>
        {isSending ? m.sending() : m.continue()}
      </Button>
    </form>
  );
}

export function CodeForm({ email }: { email: string }) {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [code, setCode] = useState("");
  const [isChecking, setIsChecking] = useState(false);
  const [error, setError] = useState<string>();
  const [wait, setWait] = useState(RESEND_SECONDS);

  useEffect(() => {
    if (wait === 0) return undefined;
    const timer = setTimeout(() => setWait((seconds) => seconds - 1), 1000);
    return () => clearTimeout(timer);
  }, [wait]);

  async function check(value: string) {
    setIsChecking(true);
    setError(undefined);
    try {
      await expectOk(
        api.POST("/auth/sign-in", { body: { email, code: value, device: browserDevice() } }),
      );
      queryClient.clear();
      await navigate({ to: "/" });
    } catch (cause) {
      setError(
        cause instanceof ApiError && cause.status === 401 ? m.wrong_code() : m.unreachable(),
      );
      setCode("");
    } finally {
      setIsChecking(false);
    }
  }

  async function resend() {
    setWait(RESEND_SECONDS);
    try {
      await sendCode(email);
    } catch {
      setError(m.unreachable());
    }
  }

  return (
    <div className="flex flex-col gap-4">
      <InputOTP
        maxLength={6}
        value={code}
        onChange={setCode}
        onComplete={(value: string) => void check(value)}
        disabled={isChecking}
        autoFocus
        inputMode="numeric"
        autoComplete="one-time-code"
        aria-label={m.six_digit_code()}
        aria-invalid={error !== undefined}
        containerClassName="justify-center"
      >
        <InputOTPGroup>
          {[0, 1, 2, 3, 4, 5].map((index) => (
            <InputOTPSlot key={index} index={index} className="size-12 text-xl" />
          ))}
        </InputOTPGroup>
      </InputOTP>
      {error && (
        <p role="alert" className="text-center text-sm text-destructive">
          {error}
        </p>
      )}
      <div className="flex items-center justify-between text-sm text-muted-foreground">
        <button
          type="button"
          className="underline-offset-2 hover:underline"
          onClick={() => void navigate({ to: "/sign-in", search: {} })}
        >
          {m.use_different_email()}
        </button>
        <button
          type="button"
          disabled={wait > 0}
          className="underline-offset-2 hover:underline disabled:no-underline disabled:opacity-60"
          onClick={() => void resend()}
        >
          {wait > 0 ? m.resend_in({ seconds: wait }) : m.resend_code()}
        </button>
      </div>
    </div>
  );
}
