# goat

A minimal, low-memory coding agent for the terminal.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/goat-agent/goat/main/install.sh | sh
```

This installs `goat` to `~/.local/bin` on macOS or Linux, x86_64 or arm64.

## Usage

```sh
goat login                 # ChatGPT (Sign in with ChatGPT or Codex login), or an OpenAI or Anthropic API key
goat                       # start a session in the current directory
goat resume                # continue the latest session in this directory
goat accounts              # list accounts
goat -m claude-opus-5-5 -a anthropic/api
goat -e high               # reasoning effort for this session
```

In a session, Enter sends and Alt+Enter or Ctrl+J starts a new line. Ctrl+C stops the running turn and Ctrl+D quits. Commands: `/model`, `/models`, `/effort`, `/account`, `/accounts`, `/exit`.

`/effort` shows the current effort and the levels the model offers, `/effort high` sets it, and `/effort default` goes back to the model's default. The effort lasts for the session. The model's summary of its reasoning is shown dimmed while it thinks.

You can add several accounts per provider and switch between them with `/account`.

## How it works

- The model has one tool, `run`. It writes JavaScript that calls `bash`, `read`, `edit` and `write` in a sandbox, so a search, an edit and a test run take one step, and only the result it returns goes into the conversation. The sandbox reaches the system only through those functions, and every shell command passes the same blocked-command check.
- Edits point at lines by hash anchors, so an edit made against an outdated read is rejected and the file is left untouched.
- The Codex login uses the backend of the Codex CLI with your ChatGPT plan. It is not a documented API, so OpenAI may change it.
- There is no sandbox and no approval prompt. A built-in check blocks a few irreversible commands, such as `rm -rf ~` or formatting a disk.
- The conversation is summarized once a request passes 200k input tokens.
- Sessions, messages and accounts are stored in `~/.goat/goat.db` (SQLite, mode 0600). Long command output is kept in `~/.goat/outputs` for 7 days.
- An `AGENTS.md` file in the working directory is added to the instructions.

## Development

```sh
cargo test --workspace
```

Contributor and agent rules are in [AGENTS.md](AGENTS.md). The code style is in [docs/style/rust.md](docs/style/rust.md).

## License

[MIT](LICENSE)
