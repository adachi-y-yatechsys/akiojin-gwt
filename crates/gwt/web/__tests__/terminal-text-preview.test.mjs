import { test } from "node:test";
import assert from "node:assert/strict";
import { parseHTML } from "linkedom";

function fixture() {
  const { document } = parseHTML("<html><body><div id='host'></div></body></html>");
  const listeners = new Set();
  const buffer = {
    baseY: 1,
    lines: ["old scrollback", "<first>", "second", "outside screen"],
    getLine(index) {
      const text = this.lines[index];
      return text === undefined ? undefined : { translateToString: () => text };
    },
  };
  const terminal = {
    rows: 2,
    buffer: { active: buffer },
    onWriteParsed(listener) {
      listeners.add(listener);
      return { dispose: () => listeners.delete(listener) };
    },
  };
  return { document, container: document.getElementById("host"), terminal, listeners };
}

test("readonly preview renders the current screen and follows parsed output and buffer switches", async () => {
  const { createTerminalTextPreview } = await import("../terminal-text-preview.js");
  const state = fixture();
  const cleanup = createTerminalTextPreview(state);
  const preview = state.container.querySelector("pre");
  assert.ok(preview);
  assert.match(preview.getAttribute("aria-label"), /read.only/i);
  assert.equal(preview.textContent, "<first>\nsecond");
  assert.equal(preview.querySelector("first"), null, "output must remain literal text");
  state.terminal.buffer.active = {
    baseY: 0,
    getLine: (index) => index === 0 ? { translateToString: () => "new screen" } : undefined,
  };
  for (const listener of state.listeners) listener();
  assert.equal(preview.textContent, "new screen\n");
  cleanup();
});

test("cleanup removes only the preview and unsubscribes parsed output", async () => {
  const { createTerminalTextPreview } = await import("../terminal-text-preview.js");
  const state = fixture();
  const sibling = state.document.createElement("span");
  state.container.appendChild(sibling);
  const cleanup = createTerminalTextPreview(state);
  assert.equal(state.listeners.size, 1);
  cleanup();
  assert.equal(state.listeners.size, 0);
  assert.equal(state.container.querySelector("pre"), null);
  assert.equal(sibling.parentElement, state.container);
});
