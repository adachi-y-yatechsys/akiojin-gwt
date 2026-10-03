/**
 * SPEC-3864 (AC-12) — install detection drives the Launch Wizard.
 *
 * Runs against a real gwt browser-server backend launched with `agy` and
 * `openclaw` removed from PATH:
 *
 *   - Antigravity CLI (installer-only route): no `Installed` entry, no Version
 *     picker, and the agent-independent setup affordance with an
 *     "Install Antigravity CLI" action is rendered.
 *   - OpenClaw (npm route): no `Installed` entry, but `latest` is offered and
 *     no setup affordance is shown.
 *
 * Like the other live specs, the suite is gated on GWT_PLAYWRIGHT_BASE_URL and
 * never submits a launch. Since SPEC-3245 Stage E removed the deprecated Intake
 * route, the wizard is opened through the normal branch Launch Wizard fixture.
 */
import { expect, test, type Page } from "@playwright/test";
import {
  acquireLiveGwtBackendLock,
  clearLiveLaunchWizard,
  gotoLiveGwt,
  openLiveGwtProject,
  openLiveLaunchWizardForBranch,
  sendLiveGwtEvent,
  type LiveLaunchWizardFixture,
} from "./_helpers/live-gwt";

const BASE = process.env.GWT_PLAYWRIGHT_BASE_URL ?? "";
// npm-routed built-in that must be absent from the backend's PATH. Defaults to
// OpenClaw (AC-12); on hosts where gwt's macOS PATH hydration re-adds a
// Homebrew-installed openclaw, point this at another npm-routed agent that is
// genuinely missing (e.g. `opencode`) — the wizard path under test is the same.
const NPM_AGENT = process.env.GWT_E2E_NPM_AGENT ?? "openclaw";

test.describe.serial("Launch Wizard agent setup affordance (live backend)", () => {
  test.skip(!BASE, "GWT_PLAYWRIGHT_BASE_URL is not set; live E2E skipped");
  test.setTimeout(120_000);

  let releaseBackendLock: (() => Promise<void>) | undefined;
  let wizardFixture: LiveLaunchWizardFixture | undefined;

  test.beforeEach(async ({ page }, testInfo) => {
    releaseBackendLock = await acquireLiveGwtBackendLock(BASE, testInfo);
    await gotoLiveGwt(page, BASE, { enableTestBridge: true });
    await keepLaunchWizardModalVisible(page);
    await openLiveGwtProject(page);
    await clearLiveLaunchWizard(page);
  });

  test.afterEach(async ({ page }) => {
    if (!releaseBackendLock) return;
    try {
      await clearLiveLaunchWizard(page);
      await wizardFixture?.cleanup();
    } finally {
      wizardFixture = undefined;
      await releaseBackendLock();
      releaseBackendLock = undefined;
    }
  });

  for (const [agentId, displayName] of [["claude", "Claude Code"], ["codex", "Codex"]]) {
    test(`${displayName} shows installed preference, detected version and setup action`, async ({ page }, testInfo) => {
      const { pageErrors, consoleErrors } = collectErrors(page);
      wizardFixture = await openLiveLaunchWizardForBranch(page);
      await enterManualSetupSettings(page);
      await selectWizardAgent(page, agentId);
      const wizard = page.locator("#wizard-modal");
      const version = wizard.getByLabel("Version", { exact: true });
      await expect(version).toHaveValue("installed");
      const installed = version.locator('option[value="installed"]');
      await expect(installed).toContainText("package fallback");
      const setup = wizard.locator(`.launch-agent-setup[data-agent-id="${agentId}"]`);
      await expect(setup).toBeVisible();
      const missing = (await installed.textContent())?.includes("not found");
      await expect(setup).toHaveAttribute("data-setup-kind", missing ? "install" : "update");
      await expect(setup.getByRole("button", { name: `${missing ? "Install" : "Update"} ${displayName}` })).toBeVisible();
      if (!missing) {
        await expect(installed).toContainText(/\d+\.\d+/);
        await expect(installed).toContainText("PATH");
      }
      await page.evaluate(() => new Promise(requestAnimationFrame));
      expect(pageErrors).toEqual([]);
      expect(consoleErrors).toEqual([]);
      await setup.scrollIntoViewIfNeeded();
      await page.screenshot({ path: testInfo.outputPath(`${agentId}-setup.png`), fullPage: true });
      await version.scrollIntoViewIfNeeded();
      await page.screenshot({ path: testInfo.outputPath(`${agentId}-installed-preference.png`), fullPage: true });
    });
  }

  test("uninstalled installer-only agent shows setup affordance and no Installed entry", async ({
    page,
  }) => {
    const { pageErrors, consoleErrors } = collectErrors(page);
    wizardFixture = await openLiveLaunchWizardForBranch(page);
    const wizard = page.locator("#wizard-modal");
    await expect(wizard).toBeVisible();
    await enterManualSetupSettings(page);

    await selectWizardAgent(page, "agy");

    const setup = wizard.locator('.launch-agent-setup[data-agent-id="agy"]');
    await expect(setup).toBeVisible({ timeout: 15_000 });
    await expect(setup).toHaveAttribute("data-setup-kind", "install");
    await expect(setup.locator(".launch-agent-setup__title")).toContainText(
      "Antigravity CLI is not installed",
    );
    await expect(setup.locator(".launch-agent-setup__detail")).toContainText(
      "antigravity.google/cli/install.sh",
    );
    await expect(
      setup.getByRole("button", { name: "Install Antigravity CLI" }),
    ).toBeVisible();

    // FR-003: neither `Installed` nor a version picker is offered.
    await expect(wizard.getByLabel("Version", { exact: true })).toHaveCount(0);
    await expect(wizard.locator('option[value="installed"]')).toHaveCount(0);

    await page.evaluate(() => new Promise(requestAnimationFrame));
    expect(pageErrors).toEqual([]);
    expect(consoleErrors).toEqual([]);
  });

  test("uninstalled npm-routed agent offers latest but not Installed", async ({
    page,
  }) => {
    const { pageErrors, consoleErrors } = collectErrors(page);
    wizardFixture = await openLiveLaunchWizardForBranch(page);
    const wizard = page.locator("#wizard-modal");
    await expect(wizard).toBeVisible();
    await enterManualSetupSettings(page);

    await selectWizardAgent(page, NPM_AGENT);

    const version = wizard.getByLabel("Version", { exact: true });
    await expect(version).toBeVisible({ timeout: 15_000 });
    await expect(version.locator('option[value="latest"]')).toHaveCount(1);
    await expect(version.locator('option[value="installed"]')).toHaveCount(0);
    await expect(version).toHaveValue("latest");
    // A configure affordance may still appear when the agent's first-time
    // setup is missing on this host; only the install kind is ruled out.
    await expect(
      wizard.locator('.launch-agent-setup[data-setup-kind="install"]'),
    ).toHaveCount(0);

    await page.evaluate(() => new Promise(requestAnimationFrame));
    expect(pageErrors).toEqual([]);
    expect(consoleErrors).toEqual([]);
  });
});

function collectErrors(page: Page) {
  const pageErrors: string[] = [];
  const consoleErrors: string[] = [];
  page.on("pageerror", (error) => pageErrors.push(String(error)));
  page.on("console", (message) => {
    if (message.type() === "error") consoleErrors.push(message.text());
  });
  return { pageErrors, consoleErrors };
}

async function selectWizardAgent(page: Page, agentId: string): Promise<void> {
  const wizard = page.locator("#wizard-modal");
  const agentField = wizard.getByLabel("Agent", { exact: true });
  await expect(agentField).toBeVisible();
  const tag = await agentField.evaluate((node) => node.tagName.toLowerCase());
  if (tag === "select") {
    await agentField.selectOption(agentId);
    await expect(agentField).toHaveValue(agentId);
    await agentField.blur();
    return;
  }
  const option = wizard.locator(`.launch-segmented__option[data-value="${agentId}"]`);
  await option.click();
  await expect(option).toHaveAttribute("aria-checked", "true");
  await page.evaluate(() => {
    const active = document.activeElement;
    if (active instanceof HTMLElement) active.blur();
  });
}

async function enterManualSetupSettings(page: Page): Promise<void> {
  const wizard = page.locator("#wizard-modal");
  const target = wizard.getByRole("radiogroup", { name: "Target" });
  if (await target.isVisible().catch(() => false)) {
    return;
  }
  await sendLiveGwtEvent(page, {
    kind: "launch_wizard_action",
    action: { kind: "use_start_method", method: "configure_and_start" },
    bounds: null,
  });
  await expect(target).toBeVisible({ timeout: 10_000 });
}

async function keepLaunchWizardModalVisible(page: Page): Promise<void> {
  await page.addStyleTag({
    content: `
      #wizard-modal[aria-hidden="false"],
      #wizard-modal.open {
        display: flex !important;
        pointer-events: auto !important;
      }
      #wizard-modal[aria-hidden="true"] {
        display: none !important;
        pointer-events: none !important;
      }
    `,
  });
}
