const terminalNode = document.getElementById("terminal"),
  stage = document.getElementById("stage");
const term = new Terminal({
  cursorBlink: true,
  scrollback: 0,
  convertEol: false,
  theme: __TERMUA_THEME__,
});
term.open(terminalNode);
const token = new URLSearchParams(location.hash.slice(1)).get("token") || "";
const ws = new WebSocket(`ws://${location.host}/ws`);
ws.binaryType = "arraybuffer";
ws.onopen = () => ws.send(JSON.stringify({ type: "authenticate", token }));
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
ws.onmessage = (e) => {
  if (e.data instanceof ArrayBuffer) {
    const bytes = new Uint8Array(e.data),
      view = new DataView(e.data);
    if (bytes[0] === 0 || bytes[0] === 2) {
      const columns = view.getUint32(1),
        rows = view.getUint32(5),
        ansi = bytes.slice(9);
      resizeTerminal(columns, rows);
      term.write(ansi);
    }
    return;
  }
  const m = JSON.parse(e.data);
  if (m.type === "access") {
    document.getElementById("status").textContent = m.control
      ? "Control granted"
      : "Read-only";
  }
};
term.onData(
  (data) =>
    ws.readyState === 1 && ws.send(JSON.stringify({ type: "input", data })),
);
terminalNode.addEventListener(
  "wheel",
  (event) => {
    event.preventDefault();
    event.stopImmediatePropagation();
  },
  { passive: false, capture: true },
);
document.getElementById("request-control").onclick = () =>
  ws.send(JSON.stringify({ type: "request_control" }));
window.addEventListener("resize", layoutTerminal);
