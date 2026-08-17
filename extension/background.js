const HOST = "com.goat.browser_host";
const CHUNK_PAYLOAD = 512 * 1024;

let port = null;
let attached = null;
const inbound = new Map();

function connect() {
  if (port) return port;
  port = chrome.runtime.connectNative(HOST);
  port.onMessage.addListener(onMessage);
  port.onDisconnect.addListener(() => {
    port = null;
    inbound.clear();
    detach().catch(() => {});
  });
  return port;
}

function send(body) {
  const live = connect();
  const encoded = JSON.stringify(body);
  const seq = Date.now();
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

async function activeTabId() {
  const [tab] = await chrome.tabs.query({ active: true, lastFocusedWindow: true });
  if (!tab) throw new Error("no active tab");
  return tab.id;
}

async function attach(tabId) {
  if (attached === tabId) return;
  await detach();
  await chrome.debugger.attach({ tabId }, "1.3");
  attached = tabId;
}

async function detach() {
  if (attached === null) return;
  const tabId = attached;
  attached = null;
  try {
    await chrome.debugger.detach({ tabId });
  } catch {
    // the tab is already gone
  }
}

async function cdp(tabId, method, params) {
  return chrome.debugger.sendCommand({ tabId }, method, params ?? {});
}

async function perform(params) {
  const tabId = await activeTabId();
  const action = params.action;
  const args = params.arguments ?? {};

  if (action === "navigate") {
    await attach(tabId);
    await cdp(tabId, "Page.enable");
    await cdp(tabId, "Page.navigate", { url: args.url });
    return { summary: `navigated to ${args.url}` };
  }
  if (action === "read_content") {
    await attach(tabId);
    const { result } = await cdp(tabId, "Runtime.evaluate", {
      expression: "document.body.innerText",
      returnByValue: true,
    });
    return { summary: "read the page text", text: result?.value ?? "" };
  }
  if (action === "click") {
    await attach(tabId);
    await cdp(tabId, "Input.dispatchMouseEvent", {
      type: "mousePressed",
      x: args.x,
      y: args.y,
      button: "left",
      clickCount: 1,
    });
    await cdp(tabId, "Input.dispatchMouseEvent", {
      type: "mouseReleased",
      x: args.x,
      y: args.y,
      button: "left",
      clickCount: 1,
    });
    return { summary: `clicked at ${args.x},${args.y}` };
  }
  if (action === "screenshot") {
    await attach(tabId);
    const shot = await cdp(tabId, "Page.captureScreenshot", {
      format: "jpeg",
      quality: 80,
    });
    return {
      summary: "captured the visible tab",
      media_type: "image/jpeg",
      image: shot.data,
    };
  }
  throw new Error(`unsupported browser action: ${action}`);
}

async function onMessage(raw) {
  const message = reassemble(raw);
  if (!message || message.type !== "browser.request") return;

  let started = false;
  try {
    started = true;
    const result = await perform(message.params ?? {});
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
  } finally {
    await detach();
  }
}

chrome.runtime.onMessage.addListener((message, _sender, respond) => {
  if (message?.t !== "status") return false;
  respond({ connected: port !== null, driving: attached !== null });
  return false;
});

chrome.action.onClicked.addListener(async (tab) => {
  await chrome.sidePanel.open({ windowId: tab.windowId });
});

chrome.runtime.onStartup.addListener(connect);
chrome.runtime.onInstalled.addListener(connect);
connect();
