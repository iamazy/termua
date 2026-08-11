const terminalNode = document.getElementById("terminal"),
  terminalFrame = document.getElementById("terminal-frame"),
  lineNumbers = document.getElementById("line-numbers"),
  stage = document.getElementById("stage"),
  statusNode = document.getElementById("status"),
  requestControl = document.getElementById("request-control");
const term = new Terminal({
  cursorBlink: true,
  scrollback: 0,
  convertEol: false,
  theme: __TERMUA_THEME__,
});
term.open(terminalNode);
const baseFontSize = term.options.fontSize;
const token = new URLSearchParams(location.hash.slice(1)).get("token") || "";
const ws = new WebSocket(`ws://${location.host}${location.pathname}ws`);
ws.binaryType = "arraybuffer";
ws.onopen = () => ws.send(JSON.stringify({ type: "authenticate", token }));
let lineNumberSignature = "",
  lineNumberDigits = 0,
  hasControl = false,
  hasMirroredSelection = false;
function renderLineNumbers(numbers) {
  const signature = numbers.join(",");
  if (signature === lineNumberSignature) return;
  lineNumberSignature = signature;
  lineNumberDigits = numbers.reduce(
    (digits, number) => Math.max(digits, number ? String(number).length : 0),
    0,
  );
  const fragment = document.createDocumentFragment();
  for (const lineNumber of numbers) {
    const number = document.createElement("span");
    number.textContent = lineNumber || "";
    fragment.appendChild(number);
  }
  lineNumbers.replaceChildren(fragment);
}
function layoutTerminal() {
  requestAnimationFrame(() => {
    const cell = term._core?._renderService?.dimensions?.css?.cell;
    if (!cell || !cell.width || !cell.height) return;
    const fontSize = term.options.fontSize,
      baseCellWidth = (cell.width * baseFontSize) / fontSize,
      baseCellHeight = (cell.height * baseFontSize) / fontSize,
      baseGutterWidth = lineNumberDigits
        ? Math.ceil(baseCellWidth * lineNumberDigits + 12)
        : 0;
    const bounds = stage.getBoundingClientRect(),
      baseTerminalWidth = Math.ceil(baseCellWidth * term.cols + 18),
      baseWantedHeight = Math.ceil(baseCellHeight * term.rows + 2),
      baseWantedWidth = baseTerminalWidth + baseGutterWidth,
      scale = Math.min(
        1.15,
        bounds.width / baseWantedWidth,
        bounds.height / baseWantedHeight,
      );
    if (Math.abs(fontSize - baseFontSize * scale) > 0.01) {
      term.options.fontSize = baseFontSize * scale;
      layoutTerminal();
      return;
    }
    const terminalWidth = Math.ceil(cell.width * term.cols + 18),
      wantedHeight = Math.ceil(cell.height * term.rows + 2),
      gutterWidth = lineNumberDigits
        ? Math.ceil(cell.width * lineNumberDigits + 12)
        : 0,
      wantedWidth = terminalWidth + gutterWidth;
    terminalFrame.style.width = `${wantedWidth}px`;
    terminalFrame.style.height = `${wantedHeight}px`;
    lineNumbers.style.width = `${gutterWidth}px`;
    lineNumbers.style.height = `${wantedHeight}px`;
    lineNumbers.style.fontSize = `${term.options.fontSize}px`;
    lineNumbers.style.setProperty("--cell-height", `${cell.height}px`);
    terminalNode.style.width = `${terminalWidth}px`;
    terminalNode.style.height = `${wantedHeight}px`;
    term.refresh(0, term.rows - 1);
  });
}
function resizeTerminal(columns, rows, lineNumbers) {
  if (term.cols !== columns || term.rows !== rows) term.resize(columns, rows);
  renderLineNumbers(lineNumbers);
  layoutTerminal();
}
ws.onmessage = (e) => {
  if (e.data instanceof ArrayBuffer) {
    const bytes = new Uint8Array(e.data),
      view = new DataView(e.data);
    if (bytes[0] === 0 || bytes[0] === 2) {
      const columns = view.getUint32(1),
        rows = view.getUint32(5),
        lineNumbers = Array.from({ length: rows }, (_, row) => {
          const number = view.getUint32(9 + row * 4);
          return number || null;
        }),
        selectionOffset = 9 + rows * 4,
        selectionColumn = view.getUint32(selectionOffset),
        selectionRow = view.getUint32(selectionOffset + 4),
        selectionLength = view.getUint32(selectionOffset + 8),
        ansi = bytes.slice(selectionOffset + 12);
      resizeTerminal(columns, rows, lineNumbers);
      term.write(ansi, () => {
        if (selectionLength) {
          term.select(selectionColumn, selectionRow, selectionLength);
          hasMirroredSelection = true;
        } else if (hasMirroredSelection) {
          term.clearSelection();
          hasMirroredSelection = false;
        }
      });
    }
    return;
  }
  const m = JSON.parse(e.data);
  if (m.type === "access") {
    hasControl = m.control;
    statusNode.textContent = hasControl ? "Control granted" : "Read-only";
    requestControl.textContent = hasControl
      ? "Release control"
      : "Request control";
    requestControl.disabled = false;
  } else if (m.type === "control_request") {
    statusNode.textContent =
      m.status === "pending"
        ? "Control request pending"
        : "Control is already in use";
  }
};
ws.onclose = () => {
  term.clear();
  lineNumbers.replaceChildren();
  statusNode.textContent = "Sharing ended";
  requestControl.disabled = true;
};
const reportActivity = () =>
  ws.readyState === WebSocket.OPEN &&
  ws.send(JSON.stringify({ type: "activity" }));
["pointerdown", "wheel", "touchstart"].forEach((eventName) =>
  document.addEventListener(eventName, reportActivity, {
    passive: true,
    capture: true,
  }),
);
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
requestControl.onclick = () => {
  if (ws.readyState !== WebSocket.OPEN) return;
  if (hasControl) ws.send(JSON.stringify({ type: "release_control" }));
  else ws.send(JSON.stringify({ type: "request_control" }));
};
window.addEventListener("resize", layoutTerminal);
