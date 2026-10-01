const SURFACES = { issues: "Issues", agents: "Agents", board: "Board", settings: "Settings" };
const PRESETS = { issues: "issue", board: "board", settings: "settings" };

// A client-local layout: move live views, never clone terminals or persist
// split dimensions as canvas geometry. The canvas remains the single-view
// layout until the later #4777 migration slices retire it.
export function createSplitSurfaces({
  document, stage, getWindows, getElement, openSurface, onChange, onLayout,
  restoreVisibility = () => {}, onAgentsHost = () => {},
}) {
  const host = document.getElementById("split-surfaces");
  const button = document.getElementById("split-view-button");
  const area = host.parentElement;
  let opened = false;
  let active = 0;
  let selections = ["issues", "board"];
  const requested = new Set();
  const mounted = new Map();
  const panes = ["Left", "Right"].map((label, index) => {
    const pane = document.createElement("section");
    pane.className = "split-pane";
    pane.setAttribute("aria-label", `${label} pane`);
    pane.innerHTML = `
      <div class="split-pane__header">
        <label>${label}<select aria-label="${label} pane surface"></select></label>
        <span class="split-pane__focus" aria-hidden="true">Active pane</span>
      </div>
      <div class="split-pane__body"></div>
      <p class="split-pane__empty" hidden></p>`;
    const select = pane.querySelector("select");
    for (const [value, text] of Object.entries(SURFACES)) {
      const option = document.createElement("option");
      option.value = value;
      option.textContent = text;
      select.appendChild(option);
    }
    select.addEventListener("change", () => choose(select.value, index));
    pane.addEventListener("pointerdown", () => focusPane(index));
    pane.addEventListener("focusin", () => focusPane(index));
    host.appendChild(pane);
    return pane;
  });

  function focusPane(index) {
    active = index;
    for (const [i, pane] of panes.entries()) pane.dataset.active = String(i === active);
    onChange(selections[active]);
  }

  function sync() {
    if (!opened) return;
    const windows = getWindows();
    const desired = new Map();
    if (!selections.includes("agents")) onAgentsHost(null);
    for (const [index, pane] of panes.entries()) {
      const surface = selections[index];
      pane.dataset.surface = surface;
      for (const option of pane.querySelectorAll("option")) {
        option.selected = option.value === surface;
        option.disabled = option.value === selections[1 - index];
        option.textContent = `${SURFACES[option.value]}${option.disabled ? " (open in other pane)" : ""}`;
      }
      if (surface === "agents") {
        pane.querySelector(".split-pane__empty").hidden = true;
        onAgentsHost(pane.querySelector(".split-pane__body"));
        continue;
      }
      const selected = windows.filter((data) => data.preset === PRESETS[surface]).slice(0, 1);
      const empty = pane.querySelector(".split-pane__empty");
      empty.hidden = selected.length > 0;
      empty.textContent = `Opening ${SURFACES[surface]}…`;
      if (selected.length) requested.delete(surface);
      else if (!requested.has(surface)) {
        requested.add(surface);
        openSurface(surface);
      }
      for (const data of selected) {
        const element = getElement(data.id);
        if (element) desired.set(data.id, { element, index });
      }
    }
    const changed = [];
    for (const [id, element] of mounted) {
      if (desired.has(id)) continue;
      // A backend removal may already have detached this view.
      if (element.parentElement) {
        stage.appendChild(element);
        restoreVisibility(id, element);
      }
      mounted.delete(id);
      changed.push(id);
    }
    for (const [id, { element, index }] of desired) {
      const body = panes[index].querySelector(".split-pane__body");
      if (element.parentElement !== body) {
        body.appendChild(element);
        changed.push(id);
      }
      // Split selection is independent of the canvas tab group's one-active
      // rule. Restore that rule when returning this live view to the canvas.
      element.hidden = false;
      mounted.set(id, element);
    }
    focusPane(active);
    if (changed.length) onLayout(changed);
  }

  function choose(surface, index = active) {
    if (!opened || !SURFACES[surface] || selections[1 - index] === surface) return false;
    selections[index] = surface;
    active = index;
    sync();
    return true;
  }

  function open(surface = "issues") {
    if (opened) return;
    selections = [SURFACES[surface] ? surface : "issues", surface === "board" ? "issues" : "board"];
    active = 0;
    opened = true;
    host.hidden = false;
    area.classList.add("is-split");
    button.setAttribute("aria-pressed", "true");
    button.textContent = "Close split";
    button.title = "Return to the canvas without closing any windows";
    sync();
  }

  function close() {
    if (!opened) return;
    const ids = [...mounted.keys()];
    for (const [id, element] of mounted) {
      if (element.parentElement) {
        stage.appendChild(element);
        restoreVisibility(id, element);
      }
    }
    mounted.clear();
    requested.clear();
    opened = false;
    onAgentsHost(null);
    host.hidden = true;
    area.classList.remove("is-split");
    button.setAttribute("aria-pressed", "false");
    button.textContent = "Split view";
    button.title = "Show two surfaces side by side";
    onLayout(ids);
    onChange(null);
  }

  return {
    open, close, sync, select: choose,
    isOpen: () => opened,
    activeSurface: () => opened ? selections[active] : null,
    containsWindow: (id) => mounted.has(id),
    focusSurface(surface) {
      const index = selections.indexOf(surface);
      if (opened && index >= 0) focusPane(index);
    },
    focusWindow(id) {
      const pane = mounted.get(id)?.closest(".split-pane");
      if (!pane) return false;
      focusPane(panes.indexOf(pane));
      return true;
    },
  };
}
