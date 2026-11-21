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
goat --continue            # continue the latest session in this directory
goat --resume <id>         # continue the given session
goat accounts              # list accounts
goat -m gpt-6.2-sol:high   # start with a model and a reasoning effort
goat -m anthropic:api/claude-opus-5-5
```

A session opens with a goat, the version, the model and the working directory. In a session, Enter sends and Alt+Enter or Ctrl+J starts a new line. Esc or Ctrl+C stops the running turn, and Ctrl+D quits. Typing `!` on an empty prompt turns it into a red shell prompt: Enter runs the line with bash in the session's directory, shows its output, and returns to the agent. The command and its output are kept in the conversation, so the next message can refer to them, but the agent does not answer until you send one. Like the agent's commands, it passes the same check that blocks commands such as `rm -rf ~`. Esc stops it, and Backspace on an empty shell prompt goes back. The prompt stays below the output while a turn runs. Enter there queues the message, and queued messages are sent one at a time after the turn ends. If the turn is stopped or fails, they go back into the prompt. Typing `/` lists the commands under the prompt: `/model`, `/effort`, `/help` and `/exit`. The first one is picked, so Enter on `/` opens `/model`. A command that changes something shows its result on one line below it, such as `└ Set effort to high`.

A model is written `provider:account/model:effort`, and the account and the effort can be left out:

| Written | Means |
|---|---|
| `gpt-6.2-sol` | this model, on the current account if it has the model |
| `gpt-6.2-sol:high` | this model with high reasoning effort |
| `openai:codex/gpt-6.2-sol` | this model on the account `openai:codex` |

`/model` and `goat -m` take this form, and `/model` and a space with Enter show the current one. `/effort high` sets the effort of the current model, and `/effort default` goes back to the model's default. A model that no account offers, or that several accounts offer when the current one does not, is an error, and so is an effort the model does not offer. The effort lasts for the session.

After `/model` or `/effort` and a space, the choices are listed under the prompt by their ids, in catalog order. What you type matches the start of an id or of a part after `-`, `.`, `:` or `/`, so `sol` finds `gpt-6.2-sol`; an exact id comes first. Results name the model as its provider shows it, such as Claude Opus 5.5. Once you type, the first entry is picked and marked with `›`. Up and Down move the pick, and Enter or Tab takes it. On a model picked with Up or Down, Left and Right lower and raise its effort. When no list is shown, Up and Down recall earlier input, and Left and Right move the cursor.

To attach an image, drop its file on the terminal, paste its path, or press Ctrl+V with an image in the clipboard. It shows as `[Image #1]` in the prompt, with a thumbnail above it in terminals that support kitty graphics placeholders, such as kitty and Ghostty. The model sees images it reads with `read`, too. Images are scaled to at most 2048 px on the long edge and kept in `goat.db`, and a request carries at most the 20 most recent ones.

The model's summary of its reasoning is shown dimmed while it thinks, and replies are styled from their Markdown.

## How it works

- The model has one tool, `run`. It writes JavaScript that calls `bash`, `read`, `edit`, `write`, `webFetch` and `webSearch` in a sandbox, so a search, an edit and a test run take one step, and only the result it returns goes into the conversation. The sandbox reaches the system only through those functions, and every shell command passes the same blocked-command check.
- `webFetch` works like the standard `fetch` but returns HTML as markdown, cut to its first and last 20 KB with the full text on disk. `webSearch` asks the current account's provider to search the web once and returns the cited pages as `{title, url, snippet}`, so it needs no extra key.
- Edits point at lines by hash anchors, so an edit made against an outdated read is rejected and the file is left untouched.
- The Codex login uses the backend of the Codex CLI with your ChatGPT plan. It is not a documented API, so OpenAI may change it.
- There is no sandbox and no approval prompt. A built-in check blocks a few irreversible commands, such as `rm -rf ~` or formatting a disk.
- The conversation is summarized once a request passes 200k input tokens.
- Sessions, messages and accounts are stored in `~/.goat/goat.db` (SQLite, mode 0600). Long command output is kept in `~/.goat/outputs` for 7 days.
- An `AGENTS.md` file in the working directory is added to the instructions.
- The models goat knows, with their names and effort levels, are listed in a catalog built into goat. A model the catalog does not have is an error until goat is updated.

## Development

```sh
cargo test --workspace
```

Contributor and agent rules are in [AGENTS.md](AGENTS.md). The code style is in [docs/style/rust.md](docs/style/rust.md).

## License

[MIT](LICENSE)
