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
curl -fsSL https://goat.sh/install.sh | sh
```

The installer drops the binary in place, walks you through your first provider key and persona, and starts the daemon in the background. `~/.goat/` holds everything from then on.

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
