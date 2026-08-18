const HOST = "com.goat.browser_host";
const CHUNK_PAYLOAD = 512 * 1024;
const PROTOCOL = "1.3";

let port = null;
let attached = null;
let current = null;
let outgoing = 0;
const inbound = new Map();

function connect() {
  if (port) return port;
  port = chrome.runtime.connectNative(HOST);
  port.onMessage.addListener(onMessage);
  port.onDisconnect.addListener(() => {
    port = null;
    inbound.clear();
    release().catch(() => {});
  });
  return port;
}

function send(body) {
  const live = connect();
  const encoded = JSON.stringify(body);
  outgoing += 1;
  const seq = outgoing;
  if (encoded.length <= CHUNK_PAYLOAD) {
    live.postMessage({ t: "message", seq, body });
    return;
  }
  const pieces = [];
  for (let at = 0; at < encoded.length; at += CHUNK_PAYLOAD) {
    pieces.push(encoded.slice(at, at + CHUNK_PAYLOAD));
  }
  pieces.forEach((piece, index) => {
    live.postMessage({
      t: "chunk",
      seq,
      index,
      total: pieces.length,
      body: piece,
    });
  });
}

function reassemble(message) {
  if (message.t === "message") return message.body;
  const group = inbound.get(message.seq) ?? new Array(message.total).fill(null);
  group[message.index] = message.body;
  if (group.some((piece) => piece === null)) {
    inbound.set(message.seq, group);
    return null;
  }
  inbound.delete(message.seq);
  return JSON.parse(group.join(""));
}

async function currentTabId() {
  if (current !== null) {
    try {
      await chrome.tabs.get(current);
      return current;
    } catch {
      current = null;
    }
  }
  const [tab] = await chrome.tabs.query({
    active: true,
    lastFocusedWindow: true,
  });
  if (!tab) throw new Error("no open tab to drive");
  current = tab.id;
  return current;
}

async function listTabs() {
  const tabs = await chrome.tabs.query({});
  return tabs.map((tab) => ({
    id: tab.id,
    url: tab.url ?? "",
    title: tab.title ?? "",
    selected: tab.id === current,
  }));
}

async function attach(tabId) {
  if (attached === tabId) return;
  await release();
  await chrome.debugger.attach({ tabId }, PROTOCOL);
  attached = tabId;
}

async function release() {
  const tabId = attached;
  attached = null;
  if (tabId === null) return;
  await chrome.debugger.detach({ tabId }).catch(() => {});
}

async function perform(params, begin) {
  switch (params.command) {
    case "cdp": {
      const tabId = await currentTabId();
      await attach(tabId);
      begin();
      const result = await chrome.debugger.sendCommand(
        { tabId },
        params.method,
        params.params ?? {},
      );
      return { reply: "cdp", result: result ?? {} };
    }
    case "tab_list":
      return { reply: "tabs", tabs: await listTabs() };
    case "tab_select": {
      begin();
      await chrome.tabs.update(params.id, { active: true });
      current = params.id;
      await attach(params.id);
      return { reply: "tabs", tabs: await listTabs() };
    }
    case "tab_close": {
      begin();
      if (params.id === attached) await release();
      await chrome.tabs.remove(params.id);
      if (params.id === current) current = null;
      return { reply: "tabs", tabs: await listTabs() };
    }
    case "tab_open": {
      begin();
      const tab = await chrome.tabs.create({ url: params.url, active: true });
      current = tab.id;
      await attach(tab.id);
      return { reply: "tabs", tabs: await listTabs() };
    }
    case "detach":
      begin();
      await release();
      return { reply: "detached" };
    default:
      throw new Error(`unsupported browser command: ${params.command}`);
  }
}

async function onMessage(raw) {
  const message = reassemble(raw);
  if (!message) return;
  if (typeof message.type === "string" && message.type.startsWith("panel.")) {
    chrome.runtime.sendMessage(message).catch(() => {});
    return;
  }
  if (message.type !== "browser.request") return;

  let started = false;
  try {
    const result = await perform(message.params ?? {}, () => {
      started = true;
    });
    send({
      type: "browser.reply",
      request_id: message.request_id,
      result,
    });
  } catch (error) {
    send({
      type: "browser.reply",
      request_id: message.request_id,
      error: { message: String(error?.message ?? error), started },
    });
  }
}

chrome.debugger.onEvent.addListener((source, method, params) => {
  if (source.tabId !== attached) return;
  send({ type: "browser.event", event: { method, params: params ?? {} } });
});

chrome.debugger.onDetach.addListener((source) => {
  if (source.tabId === attached) attached = null;
});

chrome.tabs.onRemoved.addListener((tabId) => {
  if (tabId === attached) attached = null;
  if (tabId === current) current = null;
});

chrome.runtime.onMessage.addListener((message, _sender, respond) => {
  if (message?.t === "status") {
    respond({ connected: port !== null, driving: attached !== null });
    return false;
  }
  if (message?.t === "panel") send(message.body);
  return false;
});

chrome.action.onClicked.addListener(async (tab) => {
  await chrome.sidePanel.open({ windowId: tab.windowId });
});

chrome.runtime.onStartup.addListener(connect);
chrome.runtime.onInstalled.addListener(connect);
connect();
