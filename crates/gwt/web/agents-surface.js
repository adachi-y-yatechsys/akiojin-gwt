import { surfaceForWindow } from "./surface-rail.js";

// The grid owns presentation only. Sessions and terminal runtimes remain shared
// with the existing window model; column changes never persist canvas geometry.
export function createAgentsSurface({ document, mountTerminal, sendInput, onLayout, onFocus = () => {} }) {
  const element = document.createElement("section");
  element.className = "agents-surface";
  element.setAttribute("aria-label", "Agents");
  element.innerHTML = `<header class="agents-toolbar"><h2>Agents</h2>
    <label>Columns <input type="number" min="1" max="4" step="1" value="2" aria-label="Agent columns"></label></header>
    <div class="agents-grid"></div><p class="agents-empty">No agents. Launch an agent from Issues.</p>`;
  const grid = element.querySelector(".agents-grid");
  const columns = element.querySelector("input");
  const tiles = new Map();
  function setColumns() {
    const count = Math.max(1, Math.min(4, Number.parseInt(columns.value, 10) || 1));
    columns.value = String(count);
    grid.style.gridTemplateColumns = `repeat(${count}, minmax(0, 1fr))`;
    onLayout();
  }
  columns.addEventListener("input", setColumns);
  setColumns();

  function sync(windows) {
    const agents = windows.filter((data) => surfaceForWindow(data) === "agents");
    const ids = new Set(agents.map((data) => data.id));
    for (const [id, tile] of tiles) {
      if (!ids.has(id)) { tile.remove(); tiles.delete(id); }
    }
    for (const data of agents) {
      let tile = tiles.get(data.id);
      if (!tile) {
        tile = document.createElement("article");
        tile.className = "agent-tile";
        tile.dataset.agentId = data.id;
        tile.addEventListener("pointerdown", () => onFocus(data.id));
        tile.addEventListener("focusin", () => onFocus(data.id));
        tile.innerHTML = `<header class="agent-tile__header"><h3></h3><span>Interactive</span></header>
          <div class="agent-tile__terminal terminal-root"></div>
          <form class="agent-tile__input"><textarea rows="2" aria-label="Message to agent" placeholder="Send a message"></textarea>
          <button type="submit" class="wizard-button">Send</button><span role="status"></span></form>`;
        tile.querySelector("form").addEventListener("submit", (event) => {
          event.preventDefault();
          const input = tile.querySelector(".agent-tile__input textarea");
          const status = tile.querySelector('[role="status"]');
          if (input.disabled || !input.value.trim() || !tile.dataset.sessionId) return;
          if (sendInput(tile.dataset.sessionId, input.value)) {
            input.value = "";
            status.textContent = "Message queued";
          } else status.textContent = "Unable to send. Your message is kept here.";
        });
        tiles.set(data.id, tile);
        grid.appendChild(tile);
      }
      tile.querySelector("h3").textContent = data.dynamic_title || data.title || data.id;
      tile.dataset.sessionId = data.session_id || "";
      const disabled = !data.session_id || ["stopped", "error", "interrupted"].includes(data.status);
      tile.querySelector(".agent-tile__input textarea").disabled = disabled;
      tile.querySelector("button").disabled = disabled;
      tile.querySelector(".agent-tile__header span").textContent = disabled ? "Input unavailable" : "Interactive";
      mountTerminal(data.id, tile.querySelector(".terminal-root"));
    }
    element.querySelector(".agents-empty").hidden = agents.length > 0;
  }
  return { element, sync, contains: (id) => tiles.has(id) };
}
