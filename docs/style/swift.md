# Swift style

swift-format (`.swift-format` at the repository root, strict) and the Swift 6 compiler already enforce
formatting, naming case, no force unwraps or `try!`, import order, and data-race safety. This
guide covers what no tool checks. Naming follows Apple's
[API Design Guidelines](https://www.swift.org/documentation/api-design-guidelines/); the rest
cites its source.

## Naming

- Aim for clarity at the call site: `messages.append(message)`, `client.sendMessage(path:body:)`.
- Name by role, not by type: `draft`, not `draftString`.
- Booleans read as statements: `isRunning`, `canSend`.
- Acronyms keep one case: `userID`, `serverURL`. This differs from our TypeScript (`userId`).
- Mutating and non-mutating pairs follow `sort` / `sorted`.

## Types

- Use structs by default. Use a class when the value needs identity, such as an `@Observable`
  model ([Apple](https://developer.apple.com/documentation/swift/choosing-between-structures-and-classes)).
- One primary type per file, named after it. Small private helpers may share the file.

## Optionals and errors

- Return an optional when there is one obvious way to fail; throw when callers need to know why.
  Do not return sentinel values ([Google](https://google.github.io/swift/)).
- Use untyped `throws` unless every caller must handle a closed set of errors.
- Show errors where they happened (inline under the field), not in alerts.

## Observation

- Own a model with `@State private var`. Read it through a plain property; pass bindings with
  `@Bindable`; share app-wide state with `@Environment(Type.self)`.
- Do not use `ObservableObject` or `@Published`
  ([Apple](https://developer.apple.com/documentation/swiftui/migrating-from-the-observable-object-protocol-to-the-observable-macro)).

## Concurrency

- The app target defaults to main-actor isolation. Mark types that run elsewhere `nonisolated`.
- Prefer structured concurrency: `.task`, `async let`, task groups. Use `Task {}` only to start
  async work from a synchronous UI event such as a button action.
- `@unchecked Sendable`, `nonisolated(unsafe)`, and `@preconcurrency` each need a comment naming
  what makes them safe
  ([migration guide](https://www.swift.org/migration/documentation/swift-6-concurrency-migration-guide/commonproblems)).

## Views

- Keep `body` cheap: no formatters, parsing, or other work there. Do no work in a view's `init`.
- Extract a subview when a part has its own state or repeats.
- Pass views the narrowest data they need, so they redraw only when it changes.
