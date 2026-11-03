# Web

Read [docs/style/typescript.md](../../docs/style/typescript.md) before writing TypeScript.

The web app is the browser client and what the Mac app shows in its window, and the design
reference the iPhone app follows. Every feature must be reachable by a visible control: hover and right-click
may only be shortcuts, because touch has neither.

## Map

Feature-Sliced Design, with TanStack Router's `routes/` in place of `pages/` and no `app/` layer
(`main.tsx` and `routes/__root.tsx` hold the shell). A layer imports only from layers below it:
routes → widgets → features → entities → shared. Slices do not import their siblings.

- `src/routes/` — one file per address. `_app.tsx` is the signed-in shell; dialogs live in its
  search (`?dialog=settings&tab=devices`) so a reload or a link reopens them.
- `src/widgets/` — `sidebar`, `conversation`, `settings-dialog`, `agent-dialog`.
- `src/features/` — what a user does: `sign-in`, `send-message`, `dictate`, `call`,
  `connect-integration`, `edit-agent`, `account`, `remove-device`.
- `src/entities/` — one slice per server resource (`agent`, `message`, `integration`, `device`,
  `me`): its query, its event stream, and the parts that draw it.
- `src/shared/` — `native/` (the contract with the native app showing the page, and the calls
  into it), `api/` (openapi-fetch client, Zod schemas for streamed JSON, the `dev:mock` server), `config/env.ts` (environment, checked with Zod at startup), `ui/` (shadcn components,
  copied in and ours to edit), `lib/`, `i18n/` (generated).

## Commands

```sh
bun run dev        # needs VITE_API_URL in apps/web/.env (see .env.example)
bun run dev:mock   # no server: MSW answers from src/shared/api/mock/data.ts
bun run generate   # route tree, Paraglide messages, API types; typecheck and test run it first
bun run bridge     # after changing shared/native/contract.ts; a test fails until you do
bun run typecheck
bun run test       # Vitest, for pure logic only
bunx shadcn@latest add <component>
```

## Decisions the code does not show

- Window size classes follow Material: compact under 600px shows one pane at a time and the agent
  list is its own page; medium shrinks the list to a column of faces; expanded (840px and up)
  shows the list beside the conversation and lets people collapse it. Tailwind's `sm` and `md`
  are both 600px so shadcn components follow the same line.
- The browser signs in through the same `/auth/sign-in` as the apps and keeps Better Auth's
  httpOnly cookie; it never stores the token from the body. It registers as a `web` device.
- Server data lives in TanStack Query. Event streams write into that cache, never into a second
  store, so the list and the conversation cannot disagree. Each fact has one stream: the agent
  list stream owns the list and every agent's status, a conversation stream owns its messages.
  Edits show the agent the server returned (`showAgent`) and let the stream follow. A
  conversation subscribes with `?after=<newest id>` because EventSource cannot send
  Last-Event-ID on its first connection. A refused stream rechecks the session.
- State outside the server (pending sends, the call, the theme) lives in `shared/lib/store.ts`
  stores, read in components with `use`.
- Messages the user sent stay in `features/send-message/pending.ts` until the server's copy
  arrives on the stream; `stillPending` pairs them by text and order.
- Strings live in `messages/{en,ko}.json` and reach code as Paraglide functions, so a missing key
  or argument fails `tsc`. The language comes from the browser; there is no language setting.
- The theme is kept per browser (`shared/lib/theme.ts`), and `index.html` applies it before the
  first paint.
- openapi-typescript needs TypeScript's JS API, which TypeScript 7 does not have, so this package
  pins TypeScript 5.9 for generating types and checking.
- No end-to-end tests: types catch contract breaks, and look-and-feel is checked by opening the
  app (`dev:mock` shows every state). Unit tests cover logic with edge cases, such as merging
  stream events.
- Inside the Mac app the page runs on `goat://app` and `apiBase` is `/server`, which the app
  forwards to the server with its own session token: the page never holds the token and needs no
  cookie or CORS. So sign-in sends the app's device (`host.device`), connecting a service opens
  the app's sign-in sheet instead of leaving the page, uploads go as bytes because WebKit hands
  the app no body for a form with files, and the page's bars move the window (`moveWindow`).
- Whatever the page asks of a native app is a request or event in `shared/native/contract.ts`,
  the only definition: the Mac generates its Swift types from the export, and a Windows app
  would answer the same contract.
