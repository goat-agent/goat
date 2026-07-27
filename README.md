```
   ██████╗  ██████╗   █████╗  ████████╗
  ██╔════╝ ██╔═══██╗ ██╔══██╗ ╚══██╔══╝
  ██║  ███╗██║   ██║ ███████║    ██║
  ██║   ██║██║   ██║ ██╔══██║    ██║
  ╚██████╔╝╚██████╔╝ ██║  ██║    ██║
   ╚═════╝  ╚═════╝  ╚═╝  ╚═╝    ╚═╝
```

[![ci](https://github.com/goat-agent/goat/actions/workflows/ci.yml/badge.svg)](https://github.com/goat-agent/goat/actions/workflows/ci.yml)

A single-user, single-host personal AI in Rust with two capabilities in one product:

- **agent** — an autonomous actor that lives on your chat channels, keeps lifetime memory, runs on
  a schedule, and hands coding work to the engine below.
- **code** — a terminal coding agent (full-screen TUI) backed by a resident daemon.

One binary, one daemon, one database, everything local under `~/.goat/`.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/goat-agent/goat/main/install.sh | sh
```

The installer downloads the latest release binary, verifies its checksum when local tooling is
available, and installs `goat` into `~/.goat/bin`. macOS and Linux on x86_64 and arm64; there is no
Windows build. Nothing is registered as a system service — the daemon is spawned on demand by the
first command that needs it and stays resident until `goat daemon stop`.

## Commands

```
goat                     help
goat setup               first-run setup — providers, then an optional agent
goat code                launch the coding TUI (-c resume, -w worktree)
goat code worktree       manage git worktrees
goat code search         manage search providers
goat agent add | list    manage agents
goat agent show | remove inspect or archive an agent
goat agent channel       bind an agent to a chat channel (verifies the token)
goat agent status | log  show state and recent actions
goat provider            manage LLM keys
goat daemon | remote     manage the local daemon and paired devices
goat doctor | update     diagnose config; update the binary
```

## Providers & channels

Anthropic · OpenAI · Gemini · Moonshot · Zhipu · xAI · DeepSeek · Mistral · Groq · Qwen · local.
Channels: Telegram · Discord.

## Memory

Long-term memory lives under `~/.goat/memory/` as markdown files (the prose source of truth), with
discrete claims in a bi-temporal `facts` table and a rebuildable FTS5 + sqlite-vec index. Nightly
sleep jobs distil the day into notes, extract facts, decay unrecalled ones, and log every change.

## Coding

The agent delegates multi-step coding to the code engine in-process — same daemon, no wire hop.
Delegated work runs in any project directory and streams progress, questions, and results back to
the chat, while the full transcript stays in the code thread.

## Release

Maintainers cut releases with `cargo release`. The `v{{version}}` tag triggers GitHub Actions to
build `goat-<target>.tar.gz` assets and `SHA256SUMS` for macOS and Linux on x86_64 and arm64;
`install.sh` consumes those. Every asset is then installed through `install.sh` on its own runner
and checked against the tag before the release is considered good.

## License

MIT
