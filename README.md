# goat

A minimal, low-memory coding agent for the terminal.

goat gives the model a single tool. The model writes JavaScript that runs in a sandbox, so a search, an edit and a test run take one step, and only the value the script returns enters the conversation. goat is written in Rust, and a 50-turn session stays under 40 MiB. It works with a ChatGPT plan or with OpenAI and Anthropic API keys.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/goat-agent/goat/main/install.sh | sh
```

goat runs on macOS and Linux, on x86_64 and arm64.

## Quick start

```sh
goat login            # ChatGPT, or an OpenAI or Anthropic API key
goat                  # start a session in this directory
goat --continue       # continue the latest session here
goat -m gpt-6.2-sol:high
```

In a session, type `/` to see the commands. `/help` lists the keys.

## Features

- **One tool.** The sandbox exposes `bash`, `read`, `edit`, `write`, `webFetch`, `webSearch`, `lsp`, `agent` and `mcp`.
- **Anchored edits.** Every line carries a hash anchor, and goat rejects an edit made against an outdated read.
- **Language servers.** After a run changes files, the errors it caused come back with the result. goat uses the servers already installed and downloads nothing.
- **Agents.** `agent(task)` runs a helper in a context of its own, and the helper returns only its answer.
- **MCP and plugins.** goat connects any MCP server. Built-in plugins cover Gmail, Google Workspace, Linear, Notion, Sentry, PostHog, Granola, Fireflies, and Calendar, Reminders, Contacts and computer use on macOS.
- **Skills.** goat reads the [Agent Skills](https://agentskills.io) format.
- **Rewind.** goat snapshots the files before each prompt. You can restore the conversation, the files or both.
- **No approval prompts.** A built-in check blocks irreversible commands such as `rm -rf ~`.

## Configuration

| Path | What it holds |
|---|---|
| `AGENTS.md` | Instructions. Read from `~/.goat` and from the project's root down to the working directory |
| `.goat/skills/`, `.agents/skills/` | Skills, per project or in your home |
| `.goat/agents/*.md` | Agents, per project or in `~/.goat` |
| `.goat/mcp.toml` | MCP servers, per project or in `~/.goat` |
| `~/.goat/lsp.toml` | Extra language servers |

goat keeps sessions, accounts and sign-ins in `~/.goat/goat.db`, readable only by you.

## Development

```sh
cargo test --workspace
```

The rules for contributors and coding agents are in [AGENTS.md](AGENTS.md). The code style is in [docs/style/rust.md](docs/style/rust.md).

## License

[MIT](LICENSE)
