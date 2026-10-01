import { test } from "node:test";
import assert from "node:assert/strict";
import { parseHTML } from "linkedom";
import { readFileSync } from "node:fs";

async function fixture() {
  const { createAgentsSurface } = await import("../agents-surface.js");
  const { document, window } = parseHTML("<main></main>");
  const sent = [], mounted = [];
  const surface = createAgentsSurface({ document,
    mountTerminal: (id, root) => { mounted.push([id, root]); if (!root.firstChild) root.appendChild(document.createElement("textarea")); },
    sendInput: (session, text) => { sent.push([session, text]); return true; },
    onLayout: () => {},
  });
  document.querySelector("main").appendChild(surface.element);
  return { surface, document, window, sent, mounted };
}

const agents = [
  { id: "one", preset: "codex", title: "One", session_id: "s1", status: "running" },
  { id: "two", preset: "claude", title: "Two", session_id: "s2", placement: { kind: "issue_preview" }, status: "running" },
  { id: "three", preset: "agent", title: "Three", status: "stopped" },
  { id: "pm", preset: "claude", is_pm: true },
];

test("Agents includes off-canvas agents, excludes PM and clamps the column spin control", async () => {
  const { surface, document, window, mounted } = await fixture();
  surface.sync(agents);
  assert.equal(document.querySelectorAll(".agent-tile").length, 3);
  assert.equal(mounted.length, 3);
  const columns = document.querySelector("input[type=number]");
  assert.equal(columns.getAttribute("min"), "1");
  assert.equal(columns.getAttribute("max"), "4");
  columns.value = "9";
  columns.dispatchEvent(new window.Event("input"));
  assert.equal(columns.value, "4");
  assert.equal(document.querySelector(".agents-grid").style.gridTemplateColumns, "repeat(4, minmax(0, 1fr))");
  surface.sync(agents.slice(0, 1));
  assert.equal(document.querySelectorAll(".agent-tile").length, 1);
});

test("a tile sends through its own session and disables an unavailable session", async () => {
  const { surface, document, window, sent } = await fixture();
  surface.sync(agents);
  const first = document.querySelector("[data-agent-id=one]");
  const input = first.querySelector(".agent-tile__input textarea");
  input.value = "Continue the task";
  first.querySelector("form").dispatchEvent(new window.Event("submit", { cancelable: true }));
  assert.deepEqual(sent, [["s1", "Continue the task"]]);
  assert.equal(input.value, "");
  assert.equal(document.querySelector("[data-agent-id=three] button").disabled, true);
  assert.match(first.textContent, /Interactive/);
});

test("the grid uses Operator tokens and contains the terminal inside each tile", () => {
  const css = readFileSync(new URL("../styles/components.css", import.meta.url), "utf8");
  const surface = css.slice(css.indexOf("/* Issue 4777 T-4:"));
  assert.match(surface, /grid-template-columns: repeat\(2, minmax\(0, 1fr\)\)/);
  assert.match(surface, /\.agent-tile \.agent-tile__terminal \{ position: relative; inset: auto;/);
  assert.match(surface, /var\(--color-focus-ring\)/);
  assert.doesNotMatch(surface.replace(/\/\*[\s\S]*?\*\//g, ""), /#[a-f0-9]{3,8}\b|rgba?\(/i);
});
