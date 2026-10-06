# goat

The shared TypeScript libraries that goat products build on, in one package, `@goat/sdk`. Products install a release tag from GitHub, as `"@goat/sdk": "github:goat-agent/goat#vX.Y.Z"`, never from npm.

## Setup

```sh
mise install
bun install
lefthook install
```

## Commands

- `bun run check` runs every gate: format, lint, typecheck, tests, unused-code scan. It runs typecheck and tests through Turborepo with caching.
- `bun run fix` applies formatting and autofixable lint.
- `bun run typecheck` and `bun run test` run one gate.
- `bun test <file>` runs one test file.

## Done means

- `bun run check` passes with zero warnings, and you show its output.
- New behavior has a `bun test` test next to the code.
- Never skip hooks with `--no-verify`, and never weaken a check to make it pass.

## Code

- Comment only what a reader must know and the code cannot show, such as a workaround for an external bug. Keep it as short as possible. Never restate what the code says.
- Prefer Bun built-ins such as `Bun.spawn`, `Bun.$`, `Bun.serve` and `bun:sqlite` over new dependencies.
- Keep TypeScript runnable by Node: no enums, namespaces or parameter properties, and import local files with their `.ts` extension.
- Validate data from outside the process with Zod at the boundary: model output, config files, network and IPC.

## Package

- Bun cannot install a subdirectory of a git repository, so the SDK stays one package.
- Each directory in `src/` is one module, exported as `@goat/sdk/<module>`.
- A module imports another module only through its `index.ts`, with a relative path.
- `provider` never imports an implementation. `import/no-cycle` enforces this, because implementations import values from `provider`.
- A release is a `vX.Y.Z` tag on `main`.
- Dependencies are pinned to exact versions. `bunfig.toml` refuses releases younger than three days.

## Ask first

- Adding a dependency.
- Changing the public exports.
- Editing lint, format, TypeScript, Turborepo, lefthook or mise configuration.
