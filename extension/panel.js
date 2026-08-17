const state = document.getElementById("state");

function render({ connected, driving }) {
  if (!connected) {
    state.textContent = "not connected — is the goat daemon running?";
    return;
  }
  state.textContent = driving
    ? "connected — goat is driving this tab"
    : "connected — goat can drive this browser";
}

function poll() {
  chrome.runtime
    .sendMessage({ t: "status" })
    .then((status) => render(status ?? { connected: false }))
    .catch(() => render({ connected: false }));
}

poll();
setInterval(poll, 1000);
