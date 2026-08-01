---
name: configuring-goat
description: Use this skill whenever your own configuration is in play — your watch policy, the integrations or channels you are bound to, your model, your tools, or your agent definition. Use it before touching anything under ~/.goat, and whenever the owner asks you to change what you watch, poll, notice, or report; to add, narrow, or stop a watcher; to bind or unbind an integration; to change your model or your standing instructions; or to explain why a watcher never fires. Use it even when the request sounds like a one-line edit, and even when the owner never says "config", "watch", or "file" — these settings only take effect through a validated reload, and an edit that skips it leaves the running you unchanged.
---

# Changing your own configuration

Your settings are files. You edit them with your normal file tools, then apply them with one command.

## Where things live

| Path | Holds | Applies |
|---|---|---|
| `~/.goat/agents/<slug>/config.json` | model, tools, channels, integrations, watch | `goat reload` |
| `~/.goat/agents/<slug>/agent.md` | your standing instructions | next turn |
| `~/.goat/agents/<slug>/skills/` | your skills | next turn |
| `~/.goat/config.json` | service connections, providers | `goat reload` |
| `~/.goat/credentials.json` | every secret | never edit it |

## The loop

1. Edit the file.
2. Run `goat reload` (or `goat reload -a <slug>` for one agent).
3. If it reports a problem, nothing was replaced — the running configuration is still the old one. Read the message, fix the file, run it again.
4. Tell the owner what you changed once it applies.

Validation happens at reload, not at write. A file that does not check out is inert, not fatal: you keep running with the settings you already had. So a failed reload is safe to retry, and there is no state to unwind.

## The watch section

```json
"watch": {
  "inbox": [
    { "source": "linear", "query": "assignee:@me is:open" },
    { "source": "github", "query": "is:open assignee:@me", "stream": "assigned" }
  ]
}
```

Each named workflow polls all of its sources per tick and publishes one merged update. `stream` names the diff state for a source; it defaults to the workflow name.

## Query grammar

Space-separated `key:value` pairs and bare terms. `-` negates a pair. `"quotes"` carry spaces, either whole (`"two words"`) or after a key (`label:"needs triage"`). `@me` is the one self-reference. `limit:N` caps results where the integration supports it.

**Do not guess key names.** Every integration understands a different set, and a wrong one comes back from `goat reload` with the list of keys that integration accepts. Write the query you mean, reload, and read the answer.

## Gotchas

- `config.json` rejects unknown fields outright. A misspelled key does not get ignored — the whole agent fails to load. Reload catches this before it can hurt you.
- **A missing `watch` section is not an empty one.** Leave it out and every bound integration's default watch runs. Write `"watch": {}` and nothing is watched at all.
- A `watch` section replaces the defaults. It never merges with them, so anything you still want must be listed.
- **Stream names are the key to saved diff state.** Rename a stream and its history starts over — everything that matches fires once as if new. Keep names stable unless you want exactly that.
- Two sources that land on the same integration, account, and stream collide, and the second one is dropped. Give them distinct `stream` names.
- Secrets never belong in `config.json`. Binding a channel needs a token the owner supplies at a terminal with `goat agent channel add`; you can say which binding is missing, but you cannot install the secret.
- Adding a service connection or a provider makes reload rebuild every agent, not just yours. Editing only your own `config.json` restarts only you.
- Reloading yourself is safe: the turn you are in runs to its end and still answers. What restarts is everything that listens for the *next* one, so messages that arrive during the swap can be missed. Reload when the room is quiet.

Only a new goat binary needs the daemon restarted. Everything above applies in place.
