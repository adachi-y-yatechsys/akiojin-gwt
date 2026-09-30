/* Issue #4777 T-1 — the rail picks one of four surfaces.
 *
 * The PM sits alone above a rule; Issues / Agents / Board / Settings follow
 * with an icon and a visible label. The pressed entry is the surface of the focused window, a
 * click opens (or focuses) that surface, and at 860px and below the rail is a
 * strip across the top of the canvas. Runs against the embedded frontend with
 * a stub backend, in both themes, with zero console / page errors.
 */
import { expect, test, type Page } from "@playwright/test";
import { APP_URL, installEmbeddedRoutes } from "./_helpers/embedded-frontend";

type SentMessage = {
  kind?: string;
  id?: string;
  preset?: string;
};

test.describe("Surface rail", () => {
  test.use({
    deviceScaleFactor: 1,
    viewport: { width: 1440, height: 900 },
  });

  test("selects Issues / Agents / Board / Settings and folds to a top strip when narrow", async ({
    page,
  }) => {
    const consoleErrors: string[] = [];
    const pageErrors: string[] = [];
    page.on("console", (message) => {
      if (message.type() === "error") consoleErrors.push(message.text());
    });
    page.on("pageerror", (error) => pageErrors.push(String(error)));

    await installEmbeddedRoutes(page);
    await installSurfaceRailBackend(page);
    await page.goto(APP_URL);

    const rail = page.locator("#op-rail");
    const surfaces = rail.locator(".op-rail__surface");
    const entry = (surface: string) => rail.locator(`[data-surface='${surface}']`);

    await expect(windowById(page, "board-window")).toBeVisible({ timeout: 10_000 });
    await expect(surfaces).toHaveText(["Issues", "Agents", "Board", "Settings"]);
    // The PM leads the rail, cut off from the four surfaces by a rule.
    const railOrder = await rail.evaluate((element) =>
      Array.from(element.querySelectorAll("[data-surface], .op-rail__divider")).map((node) =>
        node.classList.contains("op-rail__divider")
          ? "|"
          : (node as HTMLElement).dataset.surface ?? "",
      ),
    );
    expect(railOrder.slice(0, 6)).toEqual(["pm", "|", "issues", "agents", "board", "settings"]);
    await expect(entry("pm")).toHaveText("PM");
    for (const surface of ["issues", "agents", "board", "settings"]) {
      await expect(entry(surface).locator("svg")).toBeVisible();
    }

    // The topmost window is the Board, so the Board surface starts pressed.
    await expect(windowById(page, "board-window")).toHaveClass(/focused/);
    await expect(pressedSurfaces(page)).resolves.toEqual(["board"]);
    const railWidth = await rail.evaluate((element) => element.getBoundingClientRect().width);
    expect(railWidth).toBe(64);
    // The pressed entry carries a 2px accent edge, not only a color change.
    await expect(entry("board")).toHaveCSS("border-left-width", "2px");
    const accent = await entry("board").evaluate(
      (element) => getComputedStyle(element).borderLeftColor,
    );
    const idleEdge = await entry("issues").evaluate(
      (element) => getComputedStyle(element).borderLeftColor,
    );
    expect(accent).not.toBe(idleEdge);

    // Agents frames every agent window, but not the PM, and presses the
    // Agents entry without creating or focusing any window.
    await clearMessages(page);
    const stage = page.locator("#canvas-stage");
    const before = await stage.evaluate((element) => (element as HTMLElement).style.transform);
    await entry("agents").click();
    await expect.poll(() => pressedSurfaces(page)).toEqual(["agents"]);
    await expect
      .poll(() => stage.evaluate((element) => (element as HTMLElement).style.transform))
      .not.toBe(before);
    expect(
      (await sentMessages(page)).filter(
        (message) => message.kind === "create_window" || message.kind === "focus_window",
      ),
    ).toEqual([]);
    await expect.poll(() => inCanvasView(page, "agent-one")).toBe(true);
    await expect.poll(() => inCanvasView(page, "agent-two")).toBe(true);
    expect(await inCanvasView(page, "pm-window")).toBe(false);

    // The PM entry lands on the PM and presses itself, not Agents: the role
    // marker (is_pm), not the claude preset, decides where it belongs.
    await clearMessages(page);
    await entry("pm").click();
    await expect.poll(() => inCanvasView(page, "pm-window")).toBe(true);
    await windowById(page, "pm-window").locator(".titlebar").click();
    await expect.poll(() => pressedSurfaces(page)).toEqual(["pm"]);

    // Issues focuses the existing Issue window instead of spawning another.
    await clearMessages(page);
    await entry("issues").click();
    await expect.poll(() => focusTargets(page)).toContain("issue-window");
    await expect.poll(() => pressedSurfaces(page)).toEqual(["issues"]);
    expect(await createdPresets(page)).toEqual([]);

    // Settings has no window yet, so the rail asks the backend for one.
    await clearMessages(page);
    await entry("settings").click();
    await expect.poll(() => createdPresets(page)).toEqual(["settings"]);

    // Every entry explains itself on hover.
    for (const surface of ["issues", "agents", "board", "settings"]) {
      await expect(entry(surface)).toHaveAttribute("title", /\S/);
    }
    await expect(entry("issues")).toHaveAttribute("title", /⌘G/);

    // At 860px and below the rail is a strip across the top of the canvas.
    await page.setViewportSize({ width: 800, height: 900 });
    await expect
      .poll(() => rail.evaluate((element) => element.getBoundingClientRect().width))
      .toBe(800);
    const railBox = await rail.boundingBox();
    const canvasBox = await page.locator(".canvas-area").boundingBox();
    expect(railBox && canvasBox).toBeTruthy();
    expect(railBox!.height).toBeLessThan(160);
    expect(railBox!.y + railBox!.height).toBeLessThanOrEqual(canvasBox!.y + 1);
    await expect(entry("issues")).toHaveCSS("border-bottom-width", "2px");
    await expect(entry("issues")).toHaveCSS("border-left-width", "0px");
    const surfaceTops = await surfaces.evaluateAll((elements) =>
      elements.map((element) => Math.round(element.getBoundingClientRect().top)),
    );
    expect(new Set(surfaceTops).size).toBe(1);

    expect(pageErrors).toEqual([]);
    expect(consoleErrors).toEqual([]);
  });
});

function windowById(page: Page, id: string) {
  return page.locator(`.workspace-window[data-id='${id}']`);
}

async function inCanvasView(page: Page, id: string): Promise<boolean> {
  return page.evaluate((windowId) => {
    const canvas = document.getElementById("canvas")!.getBoundingClientRect();
    const target = document
      .querySelector(`.workspace-window[data-id='${windowId}']`)
      ?.getBoundingClientRect();
    if (!target) return false;
    return (
      target.left >= canvas.left - 1 &&
      target.right <= canvas.right + 1 &&
      target.top >= canvas.top - 1 &&
      target.bottom <= canvas.bottom + 1
    );
  }, id);
}

async function pressedSurfaces(page: Page): Promise<string[]> {
  return page.evaluate(() =>
    Array.from(document.querySelectorAll(".op-rail [data-surface][aria-pressed='true']")).map(
      (element) => (element as HTMLElement).dataset.surface ?? "",
    ),
  );
}

async function sentMessages(page: Page): Promise<SentMessage[]> {
  return page.evaluate(() => [...(((window as any).__surfaceRailSent ?? []) as SentMessage[])]);
}

async function focusTargets(page: Page): Promise<string[]> {
  return (await sentMessages(page))
    .filter((message) => message.kind === "focus_window")
    .map((message) => message.id ?? "");
}

async function createdPresets(page: Page): Promise<string[]> {
  return (await sentMessages(page))
    .filter((message) => message.kind === "create_window")
    .map((message) => message.preset ?? "");
}

async function clearMessages(page: Page): Promise<void> {
  await page.evaluate(async () => {
    await new Promise<void>((resolve) =>
      requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
    );
    (window as any).__surfaceRailSent.length = 0;
  });
}

async function installSurfaceRailBackend(page: Page): Promise<void> {
  await page.addInitScript(() => {
    const canvasWindow = (id: string, overrides: Record<string, unknown>) => ({
      id,
      title: id,
      preset: "agent",
      geometry: { x: 120, y: 100, width: 560, height: 340 },
      geometry_revision: 0,
      z_index: 1,
      status: "idle",
      minimized: false,
      maximized: false,
      pre_maximize_geometry: null,
      persist: true,
      purpose_title: null,
      dynamic_title: null,
      dynamic_title_detail: null,
      agent_id: null,
      agent_color: null,
      tab_group_id: null,
      tab_group_active: false,
      placement: { kind: "canvas" },
      ...overrides,
    });

    // Agents sit far to the right so framing them moves the camera; the PM
    // (a claude pane marked is_pm) sits far to the left so framing the
    // agents leaves it out of view.
    const windows = [
      canvasWindow("pm-window", {
        title: "Project Manager",
        preset: "claude",
        is_pm: true,
        status: "running",
        agent_id: "pm-window",
        agent_color: "yellow",
        geometry: { x: -3200, y: 80, width: 560, height: 340 },
        z_index: 2,
      }),
      canvasWindow("issue-window", {
        title: "Issues",
        preset: "issue",
        geometry: { x: 80, y: 80, width: 640, height: 420 },
        z_index: 10,
      }),
      canvasWindow("board-window", {
        title: "Board",
        preset: "board",
        geometry: { x: 760, y: 80, width: 520, height: 420 },
        z_index: 20,
      }),
      canvasWindow("agent-one", {
        status: "running",
        agent_id: "agent-one",
        agent_color: "cyan",
        geometry: { x: 2400, y: 1400, width: 560, height: 340 },
        z_index: 5,
      }),
      canvasWindow("agent-two", {
        status: "waiting",
        agent_id: "agent-two",
        agent_color: "green",
        geometry: { x: 3000, y: 1400, width: 560, height: 340 },
        z_index: 6,
      }),
    ];

    let zCounter = 20;
    let socket: FixtureWebSocket | null = null;

    const workspaceState = () => ({
      kind: "workspace_state",
      workspace: {
        app_version: "playwright",
        tabs: [
          {
            id: "tab-1",
            title: "Surface Rail Fixture",
            project_root: "/fixture",
            kind: "git",
            workspace: {
              viewport: { x: 0, y: 0, zoom: 1 },
              windows: windows.map((windowData) => ({ ...windowData })),
            },
          },
        ],
        active_tab_id: "tab-1",
        recent_projects: [],
      },
    });

    (window as any).__surfaceRailSent = [];

    class FixtureWebSocket extends EventTarget {
      static CONNECTING = 0;
      static OPEN = 1;
      static CLOSING = 2;
      static CLOSED = 3;

      url: string;
      readyState = FixtureWebSocket.CONNECTING;

      constructor(url: string) {
        super();
        this.url = url;
        socket = this;
        setTimeout(() => {
          this.readyState = FixtureWebSocket.OPEN;
          this.dispatchEvent(new Event("open"));
          this.emit(workspaceState());
        }, 0);
      }

      send(raw: string): void {
        let message: SentMessage;
        try {
          message = JSON.parse(raw) as SentMessage;
        } catch {
          return;
        }
        (window as any).__surfaceRailSent.push(message);
        if (message.kind === "focus_window") {
          const target = windows.find((windowData) => windowData.id === message.id);
          if (target) {
            zCounter += 1;
            target.z_index = zCounter;
            this.emit(workspaceState());
          }
        }
      }

      close(): void {
        this.readyState = FixtureWebSocket.CLOSED;
        this.dispatchEvent(new CloseEvent("close"));
      }

      emit(payload: unknown): void {
        setTimeout(() => {
          this.dispatchEvent(new MessageEvent("message", { data: JSON.stringify(payload) }));
        }, 0);
      }
    }

    Object.defineProperty(window, "WebSocket", {
      configurable: true,
      value: FixtureWebSocket,
    });
    (window as any).__surfaceRailSocket = () => socket;
  });
}
