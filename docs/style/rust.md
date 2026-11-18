# Rust style

Rules for all Rust code in goat. Where the Rust sources disagree, this file picks one option, and that option is binding. Tooling enforces every rule it can. The rest are checked in review.

## Principles

- Code explains itself. Use names, types and structure, not comments.
- Make invalid states unrepresentable. Use enums and newtypes, not flags and raw strings.
- Use the standard library first. Every dependency needs a reason that holds up in review.
- Memory is a feature. Borrow before cloning, stream before buffering, and keep data on disk instead of in RAM.
- Choose the obvious solution over the clever one. Use the shortest correct code, not the shortest code.
- Start with the simplest thing that works. Add structure only when a concrete need appears.

## Comments

Comments are banned. The only exceptions are the cases below.

| Allowed | Example |
|---|---|
| Lint expectations with a reason | `#[expect(clippy::cast_possible_truncation, reason = "hash is masked to 16 bits")]` |
| A why that code cannot express, with a source | `// Responses API rejects store:true on plan usage: <doc URL>` |

- **No doc comments.** Do not write `///` or `//!`. goat is not published as a library, so its API is read in code, and names, types and tests carry the meaning.
- **No comments that say what code does, and no section dividers.**
  - Do not keep commented-out code, TODOs or changelogs in code.
  - Rename, extract a function, or introduce a type instead.
  - A file that needs divider comments must be split.
- **A why-comment is one line, and it must cite an external fact.**
  - The fact is a provider doc, an issue or an RFC.
  - If the reason is internal, the code must say it instead, through a name or a type.
- **Lint suppression uses `#[expect(lint, reason = "...")]` on the smallest item.**
  - `#[allow]` is banned.
  - An `#[expect]` that no longer fires fails the build, so stale suppressions cannot stay.

## Formatting and tooling

- **`cargo fmt` formats all code** with the `rustfmt.toml` in the appendix.
  - Use stable options only.
  - Do not fight rustfmt. If a formatted line is ugly, refactor the code.
- **Import groups come in this order, with a blank line between groups:**
  1. `std`
  2. Third-party crates
  3. Workspace crates (`goat_core`)
  4. `crate::`, `super::` and `self::`

  rustfmt sorts within each group. The grouping itself is checked in review, because `group_imports` is nightly only.
- **Clippy, with the workspace configuration in the appendix, must pass with zero warnings.**
- **The toolchain is pinned in `rust-toolchain.toml`.**
  - `rust-version` in `Cargo.toml` matches it.
  - The pin moves only in a dedicated change.

## Naming

- **Case follows RFC 430.**
  - Types, traits and variants are `UpperCamelCase`.
  - Functions, variables and modules are `snake_case`.
  - Constants and statics are `SCREAMING_SNAKE_CASE`.
  - Acronyms are words: `HttpClient`, `SseEvent`, `ApiKey`, never `HTTPClient`.
- **Getters have no `get_` prefix:** write `fn name(&self)` and `fn set_name(&mut self)`.
- **Conversions follow their cost:**

  | Prefix | Cost | Example |
  |---|---|---|
  | `as_` | free borrow | `as_str` |
  | `to_` | expensive or allocating | `to_string` |
  | `into_` | consumes self | `into_inner` |

- **Constructors:** `new` builds the main value, and `from_*` and `with_*` build variants.
- **Booleans read as questions:** `is_stale`, `has_output`.
- **Do not stutter.** Name items so they read well with their module path: `session::Id`, `tool::Error`, `provider::openai::Client`.
  - Import the module, not the item, when the item name is generic. Write `use crate::tool;` and then `tool::Error`.
  - Each module's error type is called `Error`, like `std::io::Error`.
- **Length tracks scope.** Short names in closures and small blocks, descriptive names in signatures.
  - Never put a type name inside a variable name: write `lines`, not `line_vec`.
- **Test names state subject, condition and outcome:** `edit_with_stale_hash_is_rejected`.

## Crates, modules and files

- **The workspace has two crates.**
  - `goat-core` is the library: agent loop, providers, tools and sessions.
  - `goat-cli` builds the `goat` binary: the inline terminal UI and input.
  - Add a crate only when one of these is true:
    - a part is reused by another crate on its own
    - build time measurably suffers
    - a heavy dependency must become optional
- **`goat-core` has no process-global side effects.**
  - It never prints, never reads environment variables, and never touches the terminal.
  - `goat-cli` passes configuration and credentials in.
  - `goat-core` reports progress as events.
- **Modules use `foo.rs` next to `foo/`.** Never use `mod.rs`; `clippy::mod_module_files` enforces this.
- **`lib.rs` and `main.rs` hold only module declarations, re-exports and the entry point.**
- **Visibility is the smallest that compiles.**
  - Inside a crate, use `pub(crate)` by default.
  - Make an item `pub` only if `goat-cli` uses it. `unreachable_pub` catches the rest.
- **Files are grouped by concept, not by one type per file.** For example: `provider/openai.rs`, `tool/edit.rs`, `session/store.rs`.
- **Declaration order within a file:**
  1. `use`
  2. Types, consts and statics
  3. Inherent `impl` blocks, then trait `impl` blocks
  4. Free functions, in call order
  5. `#[cfg(test)] mod tests`
- **No glob imports.** The only exception is `use super::*;` in test modules.

## Types and values

- **Newtypes carry meaning:** `struct LineHash(u16)`, `struct TokenCount(u32)`. Never pass a raw `u16` or `String` whose meaning lives only in its name.
- **Enums replace boolean parameters and stringly-typed states.**

```rust
pub enum Role {
    User,
    Assistant,
    Tool,
}
```

- **Wire types are separate from domain types.**
  - Provider JSON shapes live next to the provider, for example in `provider/openai/wire.rs`, and convert into core types with `From` or `TryFrom`.
  - Core types derive `Serialize` or `Deserialize` only when they are persisted.
- **Serialized sum types use a `type` tag.**

```rust
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Entry {
    Message { role: Role, text: Box<str> },
    ToolCall { id: CallId, name: Box<str>, input: Box<str> },
}
```

- **Derives come in this order:** `Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize`. Derive only what is used.
- **Struct literals use field init shorthand.** Use `..Default::default()` only for configuration values.
- **Shared mutable state is a last resort.**
  - Pass ownership first.
  - `Rc<RefCell<_>>` and `Arc<Mutex<_>>` need a reason that holds up in review.

## Functions

- **Parameters borrow:** `&str`, `&[T]`, `&Path`. Take ownership only when the value is stored.
- **Return iterators when the caller only walks the result.** Do not collect into a `Vec` just to iterate it.
- **Handle failure first and return early.**
  - Use `let ... else` to unwrap.
  - Use let chains in place of nested `if let`.
  - Keep the happy path at the left margin.

```rust
let Some(line) = snapshot.line(hash) else {
    return Err(Error::StaleHash(hash));
};
```

- **Use a struct, not many parameters.** More than four parameters, or any two of the same type that are easy to swap, become a struct.
- **No builders.** Plain struct literals and `Default` are enough.
- **No `unwrap` or `expect` outside tests.**
  - Express invariants through types.
  - A truly unreachable branch uses `unreachable!("...")` with the invariant it relies on.

## Errors

- **`goat-core` defines one `Error` enum per module that can fail, with `thiserror`.**
  - Variants name what failed (`ReadFile`, `PatchRejected`) and carry the data the message needs.
  - Keep the cause with `#[source]` or `#[from]`. Never drop it with `map_err(|_| ...)`.
  - Use `#[from]` only when the source maps to exactly one variant.

```rust
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("read {path}")]
    ReadFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("line hash {0} is stale")]
    StaleHash(LineHash),
}
```

- **`goat-cli` uses `anyhow::Result` in `main` and command handlers only.** Add context with `.context("load config")`.
- **Error messages are lowercase, have no trailing punctuation, and never say "failed to" or "error while".** Name the action that failed.
- **Handle each error once.**
  - `goat-core` never logs. It returns errors and emits events.
  - `goat-cli` decides what the user sees.
- **A failed tool call is a result the model reads, not an `Err`.**
  - Examples are a rejected patch, a non-zero exit and a blocked command.
  - `Err` is for failures of goat itself.

## Async and concurrency

- **The runtime is tokio, with `#[tokio::main(flavor = "current_thread")]`.**
  - goat serves one user and is IO-bound.
  - A worker thread per core adds memory and buys nothing.
  - Revisit this only with a measurement.
- **Every spawned task has an owner** that awaits or aborts it, such as a `JoinSet` or a stored `JoinHandle`. No detached `tokio::spawn`.
- **Cancellation drops the future.** Ctrl-C cancels the current turn by dropping it, so every `.await` must be cancel-safe.
  - State is written only after a complete item exists.
  - Never leave a half-written entry.
- **Channels are bounded,** and the chosen capacity has a reason.
- **No lock guard is held across `.await`.** Clippy denies `await_holding_lock` and `await_holding_invalid_type`.
- **Child processes run in their own process group.**
  - On timeout or cancel, kill the whole group, not only the leader.
  - Use safe wrappers, because `unsafe_code` is forbidden.

## Memory

- **Clone only what you must keep.** `redundant_clone`, `implicit_clone` and `clone_on_ref_ptr` catch the common cases.
- **Long-lived immutable text is `Box<str>`. Shared immutable text is `Arc<str>`.**
  - `String` is for text that is still being built.
  - Use `Cow<'_, str>` when ownership is conditional.
- **Stream; never buffer a whole body.**
  - HTTP responses are parsed as SSE events as they arrive.
  - Tool output keeps a capped head and tail in memory. The full output goes to a file, and its path is returned.
- **The session transcript lives on disk** as append-only JSONL. Memory holds only the context of the next request.
- **Rendered output belongs to the terminal scrollback.** The UI never keeps its own copy.
- **Reserve capacity when the size is known.**
- **Box large futures** flagged by `large_futures`.
- **The global allocator is the system default.** Changing it needs an RSS measurement on real sessions.
- **The release profile optimizes for size:** `lto`, one codegen unit, stripped symbols, and `panic = "abort"`.

## Dependencies

- **A dependency is allowed only when one of these is true:**
  - it is more correct than code we would write, as parsers, TLS and HTTP are
  - it does something we cannot reasonably build
- **Versions live only in `[workspace.dependencies]`.** Crates write `name.workspace = true`.
- **Default features are off when the crate allows it.** Enable only the features in use.
- **`cargo::unused_dependencies` and `cargo deny` keep the tree clean.** `cargo deny` checks advisories, licenses and duplicates.

## Testing

- **Unit tests live at the end of the file in `#[cfg(test)] mod tests`.** Integration tests live in `crates/<crate>/tests/`.
- **Tests never touch a real model.**
  - Provider and loop tests replay SSE fixtures from `crates/goat-core/tests/fixtures` through a local HTTP server in `tests/support/server.rs`. Only the endpoint differs from production.
  - Fixtures follow the documented event shapes. Replacing one with a real recording is a separate, manual step.
- **Assert with `assert_eq!` and `assert_matches!`.** `assert_matches!` is imported with `use std::assert_matches;`.
- **Filesystem tests run in a fresh temporary directory,** never in the repository or the home directory.
- **One behavior per test.** The test name says it, and the body has no branching.

```rust
#[cfg(test)]
mod tests {
    use std::assert_matches;

    use super::*;

    #[test]
    fn edit_with_stale_hash_is_rejected() {
        let snapshot = Snapshot::from("fn main() {}\n");
        let edit = Edit::replace(LineHash(0xbeef), "fn main() { run() }");

        let err = snapshot.apply(&edit).unwrap_err();

        assert_matches!(err, Error::StaleHash(_));
    }
}
```

## CI gates

All of these must pass.

```sh
cargo fmt --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo deny check
```

## Sources

- Rust API Guidelines: rust-lang.github.io/api-guidelines (naming C-CASE, C-CONV, C-GETTER; errors C-GOOD-ERR)
- RFC 430 (naming conventions) and RFC 2383 (`#[expect]`)
- Rust Style Guide: doc.rust-lang.org/style-guide; rustfmt `Configurations.md`
- Edition Guide: 2024 edition; 2018 path changes (`foo.rs` without `mod.rs`)
- Clippy lint list: rust-lang.github.io/rust-clippy
- Rust release notes 1.80–1.100 (`LazyLock`, `#[expect]`, `is_none_or`, edition 2024, let chains, `assert_matches!`, `cargo::unused_dependencies`)
- `std::error::Error` documentation (message style)
- tokio documentation: runtime flavors, shared state, cancellation

## Where this file departs from the sources, and why

| Topic | Sources say | This file | Reason |
|---|---|---|---|
| Doc comments | API Guidelines: document every public item, with examples | None | goat is not published as a library. Owner's no-comment rule; names, types and tests carry the meaning |
| Lint suppression | `#[allow]` is normal | `#[allow]` banned, `#[expect]` with a reason | A suppression that stops firing fails the build instead of rotting |
| `unwrap` / `expect` | Common in docs and examples | Banned outside tests | A panic in a long session loses the user's turn |
| Runtime flavor | tokio defaults to `multi_thread` | `current_thread` | One user, IO-bound work, less memory; revisited only with a measurement |
| Import grouping | rustfmt `group_imports` | Manual four groups, checked in review | The option is nightly only, and goat builds on stable |
| Test runner | cargo-nextest is common | `cargo test` | Fewer tools; the suite is small |
| Release panics | Unwinding by default | `panic = "abort"` | Smaller binary; goat never catches panics |

## Appendix: configuration

`Cargo.toml` (workspace root):

```toml
[workspace]
members = ["crates/*"]
resolver = "3"

[workspace.package]
version = "0.0.0"
edition = "2024"
rust-version = "1.100"
license = "MIT"
publish = false

[workspace.lints.rust]
unsafe_code = "forbid"
unreachable_pub = "warn"

[workspace.lints.clippy]
pedantic = { level = "warn", priority = -1 }
must_use_candidate = "allow"
missing_errors_doc = "allow"
missing_panics_doc = "allow"
allow_attributes = "warn"
allow_attributes_without_reason = "warn"
clone_on_ref_ptr = "warn"
dbg_macro = "warn"
expect_used = "warn"
unwrap_used = "warn"
get_unwrap = "warn"
mod_module_files = "warn"
print_stdout = "warn"
print_stderr = "warn"
todo = "warn"
unimplemented = "warn"
redundant_clone = "warn"
await_holding_lock = "deny"
await_holding_invalid_type = "deny"
large_futures = "warn"
rc_buffer = "warn"
str_to_string = "warn"

[workspace.lints.cargo]
unused_dependencies = "warn"

[profile.release]
lto = true
codegen-units = 1
strip = true
panic = "abort"
```

Each crate's `Cargo.toml` inherits the lints:

```toml
[lints]
workspace = true
```

`rustfmt.toml`:

```toml
style_edition = "2024"
use_field_init_shorthand = true
use_try_shorthand = true
```

`clippy.toml`:

```toml
allow-unwrap-in-tests = true
allow-expect-in-tests = true
allow-print-in-tests = true
await-holding-invalid-types = [
    "tokio::sync::MutexGuard",
    "tokio::sync::RwLockReadGuard",
    "tokio::sync::RwLockWriteGuard",
]
```

`rust-toolchain.toml`:

```toml
[toolchain]
channel = "1.100.0"
components = ["clippy", "rustfmt"]
profile = "minimal"
```
