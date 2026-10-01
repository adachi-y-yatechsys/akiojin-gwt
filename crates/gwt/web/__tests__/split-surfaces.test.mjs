import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { parseHTML } from "linkedom";

const html = readFileSync(new URL("../index.html", import.meta.url), "utf8");

test("the title bar offers a reversible split and the split uses Operator tokens", () => {
  const { document } = parseHTML(html);
  const button = document.querySelector(".project-bar #split-view-button");
  assert.ok(button, "title bar must offer Split view");
  assert.equal(button.getAttribute("aria-pressed"), "false");
  assert.ok(document.querySelector(".canvas-area #split-surfaces[hidden]"));
  const css = readFileSync(new URL("../styles/components.css", import.meta.url), "utf8");
  assert.match(css, /\.split-pane\[data-active="true"\][\s\S]*?var\(--color-focus-ring\)/);
  assert.match(css, /\.split-pane \.resize-handle\s*\{\s*display: none !important;/);
});

async function fixture() {
  const { createSplitSurfaces } = await import("../split-surfaces.js");
  const { document, window } = parseHTML(html);
  const stage = document.getElementById("canvas-stage");
  const windows = [
    { id: "issues", preset: "issue" },
    { id: "board", preset: "board" },
    { id: "settings", preset: "settings" },
    { id: "a", preset: "agent" },
    { id: "b", preset: "codex" },
    { id: "pm", preset: "claude", is_pm: true },
  ];
  const elements = new Map(windows.map((data) => {
    const element = document.createElement("div");
    element.className = "workspace-window";
    element.dataset.id = data.id;
    element.style.left = "120px";
    element.style.width = "600px";
    stage.appendChild(element);
    return [data.id, element];
  }));
  const opened = [];
  const controller = createSplitSurfaces({
    document,
    stage,
    getWindows: () => windows,
    getElement: (id) => elements.get(id),
    openSurface: (surface) => opened.push(surface),
    onChange: () => {},
    onLayout: () => {},
  });
  return { controller, document, window, stage, windows, elements, opened };
}

test("two panes select independently, preserve live nodes and restore canvas geometry", async () => {
  const { controller, document, stage, elements } = await fixture();
  controller.open("issues");
  const panes = document.querySelectorAll(".split-pane");
  assert.equal(panes.length, 2);
  assert.equal(panes[0].dataset.surface, "issues");
  assert.equal(panes[1].dataset.surface, "board");
  assert.equal(elements.get("issues").closest(".split-pane"), panes[0]);
  controller.select("settings", 1);
  assert.equal(panes[0].dataset.surface, "issues");
  assert.equal(panes[1].dataset.surface, "settings");
  assert.equal(elements.get("board").parentElement, stage);
  assert.equal(controller.activeSurface(), "settings");
  assert.equal(controller.select("issues", 1), false, "an occupied surface cannot steal the other pane");
  assert.equal(panes[1].querySelector("option[value='issues']").disabled, true);
  controller.focusWindow("issues");
  assert.equal(panes[0].dataset.active, "true");
  controller.close();
  assert.equal(controller.isOpen(), false);
  for (const element of elements.values()) {
    assert.equal(element.parentElement, stage);
    assert.equal(element.style.left, "120px");
    assert.equal(element.style.width, "600px");
  }
});

test("Agents keeps every existing agent visible, excluding the PM, and reconciles removals", async () => {
  const { controller, elements, windows } = await fixture();
  controller.open("agents");
  assert.equal(controller.containsWindow("a"), true);
  assert.equal(controller.containsWindow("b"), true);
  assert.equal(controller.containsWindow("pm"), false);
  assert.equal(elements.get("a").closest(".split-pane"), elements.get("b").closest(".split-pane"));
  windows.splice(windows.findIndex((w) => w.id === "b"), 1);
  controller.sync();
  assert.equal(controller.containsWindow("b"), false);
});

test("a missing surface requests the existing launcher once and mounts the arriving window", async () => {
  const { controller, windows, opened } = await fixture();
  windows.splice(windows.findIndex((w) => w.id === "settings"), 1);
  controller.open("issues");
  controller.select("settings", 1);
  controller.sync();
  controller.sync();
  assert.deepEqual(opened, ["settings"]);
  windows.push({ id: "settings", preset: "settings" });
  controller.sync();
  assert.equal(controller.containsWindow("settings"), true);
});

test("Issues and Settings select their canonical views, not legacy windows sharing a rail category", async () => {
  const { controller, windows, elements, stage, document } = await fixture();
  for (const preset of ["work", "index", "agent_kanban", "profile", "branches"]) {
    const element = document.createElement("div");
    stage.appendChild(element);
    elements.set(preset, element);
    windows.unshift({ id: preset, preset });
  }
  controller.open("issues");
  assert.equal(controller.containsWindow("issues"), true);
  controller.select("settings", 1);
  assert.equal(controller.containsWindow("settings"), true);
  assert.equal(controller.containsWindow("branches"), false);
});
