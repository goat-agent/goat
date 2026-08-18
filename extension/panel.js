const log = document.getElementById("log");
const input = document.getElementById("input");
const stop = document.getElementById("stop");
const statusLine = document.getElementById("state");

const tools = new Map();
const asks = new Map();

let stream = { text: null, thinking: null };
let opened = false;
let opening = false;
let busy = false;

function send(body) {
  chrome.runtime.sendMessage({ t: "panel", body }).catch(() => {});
}

function block(className, text) {
  const node = document.createElement("div");
  node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

function append(node) {
  if (!node) return null;
  const pinned = log.scrollHeight - log.scrollTop - log.clientHeight < 40;
  log.append(node);
  if (pinned) log.scrollTop = log.scrollHeight;
  return node;
}

function toolNode(call, outcome) {
  const node = block("tool");
  node.append(block("tool-head", call.display.primary));
  if (call.display.detail) node.append(block("tool-detail", call.display.detail));
  const result = block("tool-result");
  node.append(result);
  if (outcome) {
    node.classList.toggle("failed", !outcome.ok);
    result.textContent = outcome.summary ?? "";
  } else {
    node.classList.add("running");
    tools.set(call.id, { node, result });
  }
  return node;
}

function settleTool(id, outcome) {
  const live = tools.get(id);
  if (!live) return;
  tools.delete(id);
  live.node.classList.remove("running");
  live.node.classList.toggle("failed", !outcome.ok);
  live.result.textContent = outcome.summary ?? "";
}

function askNode(call, questions) {
  const node = block("ask");
  const picked = questions.map(() => new Set());
  const typed = [];
  const button = document.createElement("button");
  button.className = "ask-send";
  button.textContent = "Answer";
  button.disabled = true;

  const answerable = () =>
    questions.every(
      (_, index) => picked[index].size > 0 || typed[index].value.trim() !== "",
    );

  questions.forEach((question, index) => {
    node.append(block("ask-question", question.question));
    if (question.options.length) {
      const row = block("ask-options");
      question.options.forEach((option) => {
        const chip = document.createElement("button");
        chip.className = "ask-option";
        chip.textContent = option.label;
        if (option.description) chip.title = option.description;
        chip.addEventListener("click", () => {
          const chosen = picked[index];
          if (chosen.has(option.label)) {
            chosen.delete(option.label);
          } else {
            if (!question.multiple) chosen.clear();
            chosen.add(option.label);
          }
          for (const other of row.children) {
            other.classList.toggle("picked", chosen.has(other.textContent));
          }
          button.disabled = !answerable();
        });
        row.append(chip);
      });
      node.append(row);
    }
    const text = document.createElement("input");
    text.className = "ask-text";
    text.type = "text";
    text.placeholder = question.options.length
      ? "or answer in your own words"
      : "your answer";
    text.addEventListener("input", () => {
      button.disabled = !answerable();
    });
    typed.push(text);
    node.append(text);
  });

  button.addEventListener("click", () => {
    const answers = questions.map((_, index) =>
      picked[index].size ? [...picked[index]].join(", ") : typed[index].value.trim(),
    );
    send({ type: "panel.answer", call, answers });
    node.classList.add("settled");
    asks.delete(call);
  });
  node.append(button);
  asks.set(call, node);
  return node;
}

function entry(item) {
  switch (item.type) {
    case "User":
      return item.system ? null : block("msg user", item.display ?? item.text);
    case "Assistant":
      return block("msg assistant", item.text);
    case "Thinking":
      return block("msg thinking", item.text);
    case "Tool":
      return toolNode(item.call, item.outcome);
    case "SubagentGroup":
      return block(
        "note",
        item.members.map((member) => member.member.label).join(", "),
      );
    case "Compaction":
      return block(
        "note",
        `compacted ${item.tokens_before} → ${item.tokens_after} tokens`,
      );
    case "Shell":
    case "Process":
      return block("shell", `$ ${item.command}\n${item.output}`);
    default:
      return null;
  }
}

function apply(event) {
  switch (event.type) {
    case "TaskStarted":
      setBusy(true);
      break;
    case "TaskDone":
      setBusy(false);
      stream = { text: null, thinking: null };
      break;
    case "UserMessage":
    case "MessageDequeued":
      if (!event.system) append(block("msg user", event.display ?? event.text));
      break;
    case "TextDelta":
      stream.thinking = null;
      if (!stream.text) stream.text = append(block("msg assistant", ""));
      stream.text.textContent += event.chunk;
      break;
    case "TextDone":
      if (!stream.text) stream.text = append(block("msg assistant", ""));
      stream.text.textContent = event.text;
      stream.text = null;
      break;
    case "ThinkingDelta":
      stream.text = null;
      if (!stream.thinking) stream.thinking = append(block("msg thinking", ""));
      stream.thinking.textContent += event.chunk;
      break;
    case "ToolStarted":
      append(toolNode(event.call, null));
      break;
    case "ToolDone":
      settleTool(event.call, event.outcome);
      break;
    case "ShellDone":
      append(block("shell", event.output));
      break;
    case "SubagentStarted":
      append(block("note", `${event.subagent_type}: ${event.label}`));
      break;
    case "SubagentDone":
      append(block("note", event.ok ? "subagent done" : "subagent failed"));
      break;
    case "AskStarted":
      append(askNode(event.call, event.questions));
      break;
    case "AskDismissed":
      asks.get(event.call)?.classList.add("settled");
      asks.delete(event.call);
      break;
    case "Error":
      append(
        block(
          "note error",
          event.hint ? `${event.message} — ${event.hint}` : event.message,
        ),
      );
      break;
    case "Notify":
      append(block(event.kind === "error" ? "note error" : "note", event.message));
      break;
    case "Retrying":
      append(
        block(
          "note",
          `retrying ${event.attempt}/${event.max_attempts}: ${event.reason}`,
        ),
      );
      break;
    case "CompactionDone":
      append(
        block(
          "note",
          `compacted ${event.tokens_before} → ${event.tokens_after} tokens`,
        ),
      );
      break;
    default:
      break;
  }
}

function restore(snapshot) {
  log.replaceChildren();
  tools.clear();
  asks.clear();
  stream = { text: null, thinking: null };
  snapshot.transcript.forEach((item) => append(entry(item)));
  snapshot.pending.forEach(apply);
  setBusy(snapshot.active !== null && snapshot.active !== undefined);
}

function setBusy(on) {
  busy = on;
  stop.hidden = !on;
}

function describe(connected, driving) {
  if (!connected) return "not connected — is the goat daemon running?";
  if (!opened) return "opening a session…";
  if (busy) return driving ? "working — driving this tab" : "working…";
  return driving ? "ready — goat is driving this tab" : "ready";
}

function tick() {
  chrome.runtime
    .sendMessage({ t: "status" })
    .catch(() => null)
    .then((status) => {
      const { connected, driving } = status ?? { connected: false, driving: false };
      if (!connected) {
        opened = false;
        opening = false;
      }
      statusLine.textContent = describe(connected, driving);
      if (connected && !opened && !opening) {
        opening = true;
        send({ type: "panel.open" });
      }
    });
}

chrome.runtime.onMessage.addListener((message) => {
  if (message?.type === "panel.item") {
    const item = message.item;
    if (item.t === "snapshot") {
      opened = true;
      opening = false;
      restore(item.state);
    } else if (item.t === "event") {
      apply(item.event);
    }
  } else if (message?.type === "panel.error") {
    opening = false;
    append(block("note error", message.message));
  }
  return false;
});

input.addEventListener("keydown", (event) => {
  if (event.key !== "Enter" || event.shiftKey || event.isComposing) return;
  event.preventDefault();
  const text = input.value.trim();
  if (!text) return;
  input.value = "";
  send({ type: "panel.submit", text });
});

stop.addEventListener("click", () => send({ type: "panel.interrupt" }));

tick();
setInterval(tick, 2000);
