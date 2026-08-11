const state = document.getElementById("state");

chrome.runtime.getPlatformInfo().then(() => {
  state.textContent = "connected — goat can drive this browser";
}).catch((error) => {
  state.textContent = `not connected: ${error}`;
});
