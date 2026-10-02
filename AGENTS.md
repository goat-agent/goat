# goat

goat is an ecosystem of agent products built with TypeScript on Bun. `apps/` holds deployable products. `packages/` holds the shared building blocks that apps assemble.

## Setup

```sh
mise install
bun install
lefthook install
```

## Commands

- `bun run check` runs every gate: format, lint, typecheck, tests, unused-code scan.
- `bun run fix` applies formatting and autofixable lint.
- `bun run typecheck` and `bun run test` run per package through Turborepo with caching. Scope them with `--filter=@goat/<name>`.
- `bun test <file>` runs one test file from inside its package.

## Done means

- `bun run check` passes with zero warnings, and you show its output.
- New behavior has a `bun test` test next to the code.
- Never skip hooks with `--no-verify`, and never weaken a check to make it pass.

## Code

- Comment only what a reader must know and the code cannot show, such as a workaround for an external bug. Keep it as short as possible. Never restate what the code says.
- Prefer Bun built-ins such as `Bun.spawn`, `Bun.$`, `Bun.serve` and `bun:sqlite` over new dependencies.
- Keep TypeScript runnable by Node: no enums, namespaces or parameter properties, and import local files with their `.ts` extension.
- Validate data from outside the process with Zod at the boundary: model output, config files, network and IPC.

## Monorepo

- Name a package after the capability it provides, as `packages/<capability>` and `@goat/<capability>`, never after the app that uses it.
- Apps only wire packages together. Logic belongs in `packages/`.
- Every app sets `"exports": {}` so nothing can import it. Internal dependencies use `workspace:*`.
- Every package has a `tsconfig.json` that extends `../../tsconfig.base.json` and `typecheck` and `test` scripts.
- Dependencies are pinned to exact versions. `bunfig.toml` refuses releases younger than three days.

## Ask first

- Adding a dependency.
- Changing a package's public exports.
- Editing lint, format, TypeScript, Turborepo, lefthook or mise configuration.
