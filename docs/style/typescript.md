# TypeScript style

`tsc` (strict, `noUncheckedIndexedAccess`, `erasableSyntaxOnly`), oxlint with type-aware rules
(`.oxlintrc.json`), and oxfmt already enforce a lot: no `any`, no unsafe assertions, no floating
promises, exhaustive switches, no enums or namespaces, no `@ts-ignore`, formatting. This guide
covers what no tool checks. Where it follows the
[Google TypeScript Style Guide](https://google.github.io/styleguide/tsguide.html), the reason is
that guide's.

## Modules

- Use named exports. Route files and tool config files (`*.config.ts`) default-export because
  their frameworks expect it; lint allows only those.
- Export only what another module imports.
- Use named imports, except for modules whose exports have generic names; import those as a
  namespace so call sites stay clear: `import * as web from "../web"`, then `web.fetch(url)`.
- Group code by file, not by objects or classes made only to hold functions.

## Naming

- `lowerCamelCase` for values and functions, `UpperCamelCase` for types. Treat acronyms as words:
  `userId`, `loadHttpUrl`. No `_` prefixes and no `I` prefix on interfaces.
- Name by role, not by implementation: `search`, `fetch`, not `firecrawlSearch`.
- Booleans read as statements: `isRunning`, `canSubmit`.

## Types

- Validate data that crosses a trust boundary at runtime with Zod: request bodies, environment,
  JSON from other services, model output. Do not cast it with `as`.
- Type object literals with an annotation (`const x: T = { … }`) or `satisfies`, not `as T`.
- Model a closed set of values as a string-literal union, or as an `as const` object when the
  values need names at runtime.
- Use the simplest type that says what you mean; derive types (`typeof table.$inferSelect`,
  `z.infer<typeof Schema>`) instead of restating them.

## Absence

- Our own types use `undefined` (optional properties, `T | undefined` returns).
- `null` appears only where the database or another system gives it to us, such as nullable
  columns. Convert at that boundary if the value travels further.

## Errors

- Throw `Error` or a subclass, never a string. When rethrowing, pass the original as `cause`.
- Keep `try` blocks around only the call that can fail.
- An empty `catch` needs a comment saying why ignoring the error is safe.
- Name error classes after what happened (`LeaseLost`), not after where they were thrown.

## Async

- Mark a deliberately unawaited promise with `void`, and only when the callee handles its own
  errors.
- Give functions with side effects a block body, even when one line would fit; logging and
  checks get added there later.

## Functions and classes

- Prefer module-level functions. Use a class when instances carry state or identity, and for
  error types.
- Use `function` declarations for top-level functions; use arrow functions for callbacks and for
  closures inside a function.
- Declare the return type of exported functions that other modules build on, such as agent runs;
  the signature is the contract. Small helpers may rely on inference.

## Comments

- Write a comment only for what the code cannot say: an invariant, a reason, a constraint from
  outside the codebase. Keep it short.
- Use `/** */` on an export only when it adds something the name and types do not. Do not repeat
  types in JSDoc.
