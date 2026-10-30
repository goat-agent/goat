# Goat

Goat is a personal AI agent. Each account has agents; each agent has one conversation that the
user reaches from the iPhone app (later macOS and web). The server runs the agent and its tools.

## Map

- `apps/server` — Bun, Hono, Drizzle on Postgres, Better Auth, Vercel AI SDK. See
  [apps/server/AGENTS.md](apps/server/AGENTS.md).
- `apps/ios` — SwiftUI app for iOS 26+, and the `GoatAPI` Swift package generated from the
  server's OpenAPI document. See [apps/ios/AGENTS.md](apps/ios/AGENTS.md).
- `docs/style` — style guides. Read [typescript.md](docs/style/typescript.md) before writing
  TypeScript and [swift.md](docs/style/swift.md) before writing Swift.
- `apps/*` holds what is deployed; `packages/*` is for code two apps share and is created only
  when that happens.

## Commands

```sh
bun install
docker compose up -d                 # local Postgres
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
- Never log message text, fetched page content, tokens, or sign-in codes.
- Comments explain what the code cannot: invariants, reasons, constraints from outside. They do
  not narrate the code.
- Commit messages are `type: description`, lowercase, one line (`feat`, `fix`, `chore`,
  `refactor`, `docs`, `test`, `perf`).
