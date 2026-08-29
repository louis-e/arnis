// Stream Mode panel.
//
// The panel is a modal in the main Arnis window, opened by main.js when the
// server starts and closed when it stops. This module owns only what is inside
// it: the status poll, four bits of text and one CSS custom property.
//
// Polling runs only while the panel is open, so a closed panel costs nothing.

const invoke =
  window.__TAURI__ && window.__TAURI__.core
    ? window.__TAURI__.core.invoke
    : null;

const POLL_MS = 1000;

let pollTimer = null;

function el(id) {
  return document.getElementById(id);
}

function panel() {
  return document.querySelector('#stream-modal .stream-content');
}

function setState(state) {
  const content = panel();
  if (content && content.dataset.streamState !== state) {
    content.dataset.streamState = state;
  }
}

function setText(node, text) {
  if (node && node.textContent !== text) {
    node.textContent = text;
  }
}

function count(n) {
  return Number(n || 0).toLocaleString();
}

function render(status) {
  setState(
    status.requestsInFlight > 0
      ? 'busy'
      : status.clientConnected
        ? 'connected'
        : 'waiting'
  );

  setText(el('stream-address'), `127.0.0.1:${status.port}`);

  if (status.clientConnected) {
    const name = status.clientName || 'a client';
    const version = status.clientVersion ? ` ${status.clientVersion}` : '';
    setText(el('stream-client'), `Connected: ${name}${version}`);
  } else {
    setText(el('stream-client'), 'Waiting for Minecraft to connect...');
  }

  setText(
    el('stream-counters'),
    `${count(status.chunksServed)} chunks · ${count(status.tilesGenerated)} tiles`
  );
}

// The server is gone. Normally the panel is about to be closed by main.js, so
// this is the brief moment in between — and the honest display if it is not.
function renderStopped(message) {
  setState('stopped');
  setText(el('stream-client'), message);
  setText(el('stream-counters'), '');
}

async function poll() {
  if (!invoke) {
    renderStopped('Not running inside Arnis.');
    return;
  }
  try {
    render(await invoke('gui_stream_status'));
  } catch (error) {
    renderStopped(
      typeof error === 'string' ? error : 'Stream mode is not running.'
    );
  }
}

/// Show the panel and start polling. Safe to call when it is already open.
export function openStreamPanel(port) {
  const modal = el('stream-modal');
  if (!modal) return;

  // Show the bound port immediately rather than waiting a second for the first
  // poll, so the panel never briefly displays a stale one.
  if (Number.isFinite(port)) {
    setText(el('stream-address'), `127.0.0.1:${port}`);
  }
  setState('waiting');
  setText(el('stream-counters'), '');
  modal.style.display = 'flex';

  poll();
  if (pollTimer === null) {
    pollTimer = setInterval(poll, POLL_MS);
  }
}

/// Hide the panel and stop polling. Does NOT stop the server; main.js owns that
/// so there is one place where stream mode is started and stopped.
export function hideStreamPanel() {
  const modal = el('stream-modal');
  if (modal) {
    modal.style.display = 'none';
  }
  if (pollTimer !== null) {
    clearInterval(pollTimer);
    pollTimer = null;
  }
}
