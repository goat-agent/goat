# Goat

Goat is a personal AI agent. Each account has agents; each agent has one conversation that the
user reaches from the iPhone and Mac apps (later web). The server runs the agent and its tools,
and the agent can use a signed-in Mac through computer use.

## Map

- `apps/server` — Bun, Hono, Drizzle on Postgres, Better Auth, Vercel AI SDK. See
  [apps/server/AGENTS.md](apps/server/AGENTS.md).
- `apps/ios` — SwiftUI app for iOS 26+. See [apps/ios/AGENTS.md](apps/ios/AGENTS.md).
- `apps/macos` — SwiftUI app for macOS 26+, distributed outside the App Store, and `Computer`,
  the package that runs computer use. See [apps/macos/AGENTS.md](apps/macos/AGENTS.md).
- `apps/shared` — Swift both Apple apps build from: `GoatKit` (screens and models, compiled into
  each app) and `GoatAPI` (the client generated from the server's OpenAPI document).
- `docs/style` — style guides. Read [typescript.md](docs/style/typescript.md) before writing
  TypeScript and [swift.md](docs/style/swift.md) before writing Swift.
- `apps/*` holds what is deployed, plus `apps/shared` for the Swift the Apple apps share;
  `packages/*` is for TypeScript two apps share and is created only when that happens.

## Commands

```sh
bun install
docker compose up -d                 # local Postgres and file storage (RustFS)
bun run typecheck                    # tsc for every workspace
bun run lint                         # oxlint, type-aware; warnings fail
bun run format                       # oxfmt; format:check only checks
bun run test                         # server tests against a real Postgres
```

App-specific commands (migrations, OpenAPI export, iOS build and lint) are in each app's AGENTS.md.
Before finishing, run the checks for every app you touched.

## Decisions that the code does not show

- Name modules by role, not by vendor or tool: `lib/auth.ts` holds Better Auth, and the search
  backend lives in `lib/web/firecrawl.ts`, not `lib/firecrawl`. Vendor terms stay inside the
  vendor's file, so swapping the vendor never renames the module. Files a tool generates keep the
  tool's defaults (`drizzle/`, `src/db/schema/auth.ts`).
- Add an abstraction only where a change is already planned or a second implementation or caller
  exists, as with model choice in `lib/ai/providers.ts`. Do not build features ahead of a
  decision.
- Keep one machine's circumstances out of the repo. Ports, proxies, and keys belong in ignored
  files: `.env` at the root (Docker Compose), `apps/server/.env`, `apps/server/.env.test.local`.
- The server's Zod route schemas are the only API contract. The iOS client is generated from the
  exported document, so route changes are not done until the document is exported again.
- Enum-like fields in the API accept any string as well (`openEnum`), so app builds already in
  users' hands keep decoding values added later.
- Users never see model or provider names.
- Users see the language their device is set to; there is no language setting inside the apps.
  The server answers in the language of the request's `Accept-Language` (`lib/i18n.ts`), and
  English is the fallback everywhere. Do not store text in a user's language: one account is
  read from devices set to different languages.
- Never log message text, fetched page content, tokens, or sign-in codes.
- Comments explain what the code cannot: invariants, reasons, constraints from outside. They do
  not narrate the code. Doc comments (`///`, `/** */`) follow the same rule. If deleting a comment
  would cost the reader nothing, do not write it.
- Commit messages are `type: description`, lowercase, one line (`feat`, `fix`, `chore`,
  `refactor`, `docs`, `test`, `perf`).

## Talking to the user

Replies read like a note from a concise teammate.

- Lead with the answer or the outcome. No preamble, no recap of earlier turns.
- Keep it short by saying less, not by compressing: full sentences, no arrows, abbreviations, or
  fragments. One topic per reply; go long only when asked.
- Write short sentences in plain words. In Korean, use 해요체, verbs over nouns, active over
  passive, and no 번역투 (`~에 대해`, `~하기 위해`, `~에 의해`).
- Write prose by default. Use a list only for items that are truly separate, and do not mix bold
  with code.
- Ask at most one question, last, with the option you lean toward and why.
- While working, say in one sentence what you are about to do, and update only when something
  important changes.

<example>
로그인 코드가 안 오던 건 서버의 메일 키가 만료돼서였어요. `apps/server/.env`에 새 키를 넣고
서버를 다시 띄우면 와요.
</example>
