const terminalNode = document.getElementById("terminal"),
  stage = document.getElementById("stage");
const term = new Terminal({
  cursorBlink: true,
  scrollback: 10000,
  convertEol: false,
  theme: __TERMUA_THEME__,
});
term.open(terminalNode);
const token = new URLSearchParams(location.hash.slice(1)).get("token") || "";
const ws = new WebSocket(`ws://${location.host}/ws`);
ws.binaryType = "arraybuffer";
let latestSnapshot = new Uint8Array(),
  historyChunks = [],
  historyBefore = 0,
  historyLoading = false,
  historyAwaitingSnapshot = false;
ws.onopen = () => ws.send(JSON.stringify({ type: "authenticate", token }));
function joined(chunks) {
  const size = chunks.reduce((sum, chunk) => sum + chunk.length, 0),
    result = new Uint8Array(size);
  let offset = 0;
  for (const chunk of chunks) {
    result.set(chunk, offset);
    offset += chunk.length;
  }
  return result;
}
function layoutTerminal() {
  requestAnimationFrame(() => {
    const cell = term._core?._renderService?.dimensions?.css?.cell;
    if (!cell || !cell.width || !cell.height) return;
    const bounds = stage.getBoundingClientRect(),
      wantedWidth = Math.ceil(cell.width * term.cols + 18),
      wantedHeight = Math.ceil(cell.height * term.rows + 2),
      scale = Math.min(
        1.15,
        bounds.width / wantedWidth,
        bounds.height / wantedHeight,
      );
    terminalNode.style.width = `${wantedWidth}px`;
    terminalNode.style.height = `${wantedHeight}px`;
    terminalNode.style.transform = `scale(${scale})`;
    term.refresh(0, term.rows - 1);
  });
}
function resizeTerminal(columns, rows) {
  if (term.cols !== columns || term.rows !== rows) term.resize(columns, rows);
  layoutTerminal();
}
function rebuild() {
  term.write("", () => {
    term.reset();
    term.write(joined([...historyChunks, latestSnapshot]), () => {
      historyLoading = false;
      layoutTerminal();
    });
  });
}
function loadHistory() {
  if (historyLoading || historyBefore === 0 || ws.readyState !== 1) return;
  historyLoading = true;
  ws.send(JSON.stringify({ type: "history", before: historyBefore }));
}
ws.onmessage = (e) => {
  if (e.data instanceof ArrayBuffer) {
    const bytes = new Uint8Array(e.data),
      view = new DataView(e.data);
    if (bytes[0] === 0 || bytes[0] === 2) {
      const columns = view.getUint32(1),
        rows = view.getUint32(5),
        ansi = bytes.slice(9);
      resizeTerminal(columns, rows);
      if (bytes[0] === 0) {
        latestSnapshot = ansi;
        if (historyAwaitingSnapshot) {
          historyAwaitingSnapshot = false;
          rebuild();
        } else term.write(ansi);
      } else if (!historyAwaitingSnapshot) term.write(ansi);
    } else if (bytes[0] === 1) {
      historyBefore = Number(view.getBigUint64(1));
      historyChunks.unshift(bytes.slice(9));
      historyAwaitingSnapshot = true;
    }
    return;
  }
  const m = JSON.parse(e.data);
  if (m.type === "access") {
    historyBefore = m.history_before;
    document.getElementById("status").textContent = m.control
      ? "Control granted"
      : "Read-only";
  }
};
term.onData(
  (data) =>
    ws.readyState === 1 && ws.send(JSON.stringify({ type: "input", data })),
);
term.onScroll((position) => {
  if (position < 20) loadHistory();
});
document.getElementById("terminal").addEventListener(
  "wheel",
  (event) => {
    if (event.deltaY < 0) loadHistory();
  },
  { passive: true },
);
document.getElementById("request-control").onclick = () =>
  ws.send(JSON.stringify({ type: "request_control" }));
window.addEventListener("resize", layoutTerminal);
