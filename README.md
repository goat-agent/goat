```
   ██████╗  ██████╗   █████╗  ████████╗
  ██╔════╝ ██╔═══██╗ ██╔══██╗ ╚══██╔══╝
  ██║  ███╗██║   ██║ ███████║    ██║
  ██║   ██║██║   ██║ ██╔══██║    ██║
  ╚██████╔╝╚██████╔╝ ██║  ██║    ██║
   ╚═════╝  ╚═════╝  ╚═╝  ╚═╝    ╚═╝
```

Autonomous personal AI agent. Single user, single host. Runs personas across chat channels, ticks itself, evaluates its own output, and keeps everything local under `~/.goat/`.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/goat-agent/goat/main/install.sh | sh
```

The installer downloads the latest GitHub Release binary, verifies the release checksum when local tooling is available, walks you through first-time setup on an interactive fresh install, and installs a user daemon in the background. `~/.goat/` holds everything from then on.

## Providers

Anthropic · OpenAI · Gemini · Moonshot · Zhipu

## Channels

Telegram · Discord

## Commands

```
goat doctor       parse every config file and report
goat provider     manage LLM keys
goat persona      manage personas
goat skill        inspect agent skills
```

## License

MIT
