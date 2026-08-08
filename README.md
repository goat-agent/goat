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
Windows build. Nothing is registered as a system service — `goat code` spawns the daemon on demand,
`goat daemon start` brings it up on its own, and either way it detaches from the terminal and stays
resident until `goat daemon stop`. A daemon left over from an older build is replaced automatically
while it is idle, and reported instead of replaced while it is busy.

## Commands

```
goat                     help
goat setup               first-run setup — providers, then an optional agent
goat code                launch the coding TUI (-c resume, -w worktree)
goat code worktree       manage git worktrees
goat code search         manage search providers
goat code session        list or end live coding sessions
goat agent add | list    manage agents
goat agent show | remove inspect or archive an agent
goat agent channel       bind an agent to a chat channel (verifies the secrets)
goat agent status | log  show state and recent actions
goat provider            manage LLM keys
goat daemon              start | stop | status | serve the local daemon
goat remote              manage paired devices
goat doctor | update     diagnose config; update the binary
```

## Providers, channels & integrations

Provider ids, as typed into `goat provider login`: `anthropic`, `openai`, `openai-codex`, `gemini`,
`xai`, `kimi`, `kimi-code`, `zai`, `zai-coding`, `deepseek`, `qwen`, `minimax`, `mistral`, `groq`,
`openrouter`, `vercel`, and the local trio `ollama`, `lmstudio`, `llama-cpp`. `openai-codex`,
`kimi-code` and `zai-coding` are the subscription/coding-plan surfaces; `openrouter` and `vercel`
are aggregators.
Any other OpenAI-compatible endpoint (LiteLLM, vLLM, a corporate gateway) becomes a first-class
provider with `goat provider add <name> --endpoint <url> [--key <key>]`; its models are discovered
live and addressed as `<name>/<model>`. Remove it with `goat provider remove <name>`.
Channels: Discord, Slack — bind with `goat agent channel add`. A channel is where the agent *is*:
it holds a resident connection under its own bot identity, and people talk to it there. Slack needs
two tokens (a `xoxb-` bot token to speak, a `xapp-` app-level token to open the socket); the setup
text printed by `goat agent channel add slack` carries the app manifest.

Integrations: GitHub, Langfuse, Linear, Notion, PostHog, Sentry, Slack, Tiro — connect with
`goat integration add`, bind per agent with `goat agent integration add`. GitHub reads its
credential from the `gh` cli, so run `gh auth login` first. Langfuse takes the project's public and
secret key joined by a colon, and a `host` in its binding reaches a cloud region or a self-hosted
instance.

Slack appears in both lists and they are different things. The **channel** is the bot people address;
the **integration** reaches into Slack as *you* (`xoxp-` user token) to search and read history. The
two token types have disjoint capabilities, so neither replaces the other — add both if you want both.

## Memory

Long-term memory lives under `~/.goat/memory/` as markdown files (the prose source of truth), with
discrete claims in a bi-temporal `facts` table and a rebuildable FTS5 + sqlite-vec index. Nightly
sleep jobs distil the day into notes, extract facts, decay unrecalled ones, and log every change.

## Coding

The agent delegates multi-step coding to the code engine in-process — same daemon, no wire hop.
Delegated work runs in any project directory and streams progress, questions, and results back to
the chat, while the full transcript stays in the code conversation.

## Release

Maintainers cut releases with `cargo release`. The `v{{version}}` tag triggers GitHub Actions to
build `goat-<target>.tar.gz` assets and `SHA256SUMS` for macOS and Linux on x86_64 and arm64;
`install.sh` consumes those. Every asset is then installed through `install.sh` on its own runner
and checked against the tag before the release is considered good.

## License

MIT
