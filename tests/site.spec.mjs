import { expect, test } from "@playwright/test";

test.describe("landing page", () => {
  test("answers what, for whom and status in the hero", async ({ page }) => {
    await page.goto("/");
    await expect(page).toHaveTitle("The missing front door to Java | jx");
    await expect(page.locator("h1")).toHaveText("The missing front door to Java.");
    await expect(page.locator(".hero-sub")).toContainText("A proposed command-line tool");
    await expect(page.locator(".hero .badge")).toContainText("Design preview, not yet released");
    await expect(page.locator(".status-strip")).toContainText("Not scheduled");
    await expect(page.locator("main")).toHaveAttribute("data-status", "draft");
  });

  test("has no CSP or runtime errors", async ({ page }) => {
    const messages = [];
    page.on("console", (message) => {
      if (message.type() === "error") messages.push(message.text());
    });
    page.on("pageerror", (error) => messages.push(error.message));
    await page.goto("/");
    await page.waitForLoadState("networkidle");
    expect(messages.filter((message) => /content security policy/i.test(message))).toEqual([]);
    expect(messages).toEqual([]);
  });

  test("keeps the static transcript when motion is reduced", async ({ page }) => {
    await page.goto("/");
    const staticTranscript = page.locator(".terminal-static");
    await expect(staticTranscript).toBeVisible();
    await expect(staticTranscript).toContainText("$ jx new hello && cd hello");
    await expect(staticTranscript).toContainText("Hello, world!");
    await expect(page.locator(".terminal-animated")).toBeHidden();
    await expect(page.locator("[data-terminal-pause]")).toBeHidden();
    await expect(page.locator("[data-terminal-all]")).toBeHidden();
  });

  test("the first tab stop is the skip link and it moves focus", async ({ page }) => {
    await page.goto("/");
    await page.keyboard.press("Tab");
    await expect(page.locator(".skip-link")).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(page.locator("#main")).toBeFocused();
  });

  test("the FAQ works as native details", async ({ page }) => {
    await page.goto("/");
    const first = page.locator(".faq details").first();
    await expect(first).not.toHaveAttribute("open", "");
    await first.locator("summary").click();
    await expect(first).toHaveAttribute("open", "");
    await expect(first.locator("p")).toHaveText("No. There is no release, and the V1 plan is public.");
  });

  test("does not overflow at 375 pixels", async ({ page }) => {
    await page.setViewportSize({ width: 375, height: 812 });
    await page.goto("/");
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  });
});

test.describe("terminal animation", () => {
  test.use({ reducedMotion: "no-preference" });

  test("plays once and can be shown in full", async ({ page }) => {
    await page.goto("/");
    const animated = page.locator(".terminal-animated");
    await page.locator("[data-terminal]").scrollIntoViewIfNeeded();
    await expect(page.locator("[data-terminal-all]")).toBeVisible();
    await expect(animated).toBeVisible();
    await expect(animated).toContainText("Hello, world!", { timeout: 12_000 });
    await expect(page.locator("[data-terminal-all]")).toBeHidden();
    await expect(page.locator(".terminal-static")).toHaveClass(/visually-hidden/);
  });

  test("show all restores the static transcript", async ({ page }) => {
    await page.goto("/");
    await page.locator("[data-terminal]").scrollIntoViewIfNeeded();
    await page.locator("[data-terminal-all]").click();
    await expect(page.locator(".terminal-static")).toBeVisible();
    await expect(page.locator(".terminal-animated")).toBeHidden();
    await expect(page.locator("[data-terminal-all]")).toBeHidden();
  });
});

test.describe("theme", () => {
  test("toggles, persists and keeps its accessible name in sync", async ({ page }) => {
    await page.goto("/");
    const toggle = page.locator(".theme-toggle");
    await expect(toggle).toHaveText("Dark theme");
    await expect(toggle).toHaveAttribute("aria-pressed", "false");
    await toggle.click();
    await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
    await expect(toggle).toHaveText("Light theme");
    await expect(toggle).toHaveAttribute("aria-pressed", "true");
    await expect(toggle).toHaveAttribute("aria-label", "Switch to light theme");
    await page.reload();
    await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
    await expect(page.locator(".theme-toggle")).toHaveText("Light theme");
  });
});

test.describe("without JavaScript", () => {
  test.use({ javaScriptEnabled: false });

  test("the page stays readable and navigable", async ({ page }) => {
    await page.goto("/");
    await expect(page.locator("h1")).toBeVisible();
    await expect(page.locator(".terminal-static")).toBeVisible();
    await expect(page.locator(".theme-toggle")).toBeHidden();
    await page.locator(".faq summary").first().click();
    await expect(page.locator(".faq details").first()).toHaveAttribute("open", "");
  });
});

test.describe("docs", () => {
  test("share a sidebar, a status badge and a draft command reference", async ({ page }) => {
    await page.goto("/docs/commands.html");
    await expect(page.locator(".docs-nav a[aria-current='page']")).toHaveText("Commands");
    await expect(page.locator(".doc-head .badge")).toContainText("Draft");
    await expect(page.locator("tbody tr")).toHaveCount(17);
    await expect(page.locator("p.note")).toContainText("Every command in this table is part of the V1 plan");
  });

  test("copy the illustrative manifest and announce it", async ({ page, context }) => {
    await context.grantPermissions(["clipboard-read", "clipboard-write"]);
    await page.goto("/docs/concepts.html");
    await page.locator("[data-copy='manifest-example']").click();
    await expect(page.locator("[data-copy-status='manifest-example']")).toContainText("Copied");
    const text = await page.evaluate(() => navigator.clipboard.readText());
    expect(text).toContain('name = "hello"');
    expect(text).toContain("<version>");
  });

  test("does not overflow at 375 pixels", async ({ page }) => {
    await page.setViewportSize({ width: 375, height: 812 });
    await page.goto("/docs/glossary.html");
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  });
});

test.describe("system dark theme", () => {
  test.use({ colorScheme: "dark" });

  test("follows the system preference and labels the toggle", async ({ page }) => {
    await page.goto("/");
    await expect(page.locator(".theme-toggle")).toHaveText("Light theme");
    await expect(page.locator(".theme-toggle")).toHaveAttribute("aria-pressed", "true");
    const background = await page.evaluate(() => getComputedStyle(document.body).backgroundColor);
    expect(background).toBe("rgb(14, 17, 22)");
  });
});

test("the 404 page is served with a clear way back", async ({ page }) => {
  const response = await page.goto("/missing-page");
  expect(response.status()).toBe(404);
  await expect(page.locator("h1")).toHaveText("This page does not exist.");
  await expect(page.locator("a", { hasText: "Go to the landing page" })).toBeVisible();
});
