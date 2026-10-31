import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "@playwright/test";

const pages = [
  "/",
  "/vision.html",
  "/roadmap.html",
  "/privacy.html",
  "/accessibility.html",
  "/404.html",
  "/docs/quickstart.html",
  "/docs/concepts.html",
  "/docs/commands.html",
  "/docs/maven-gradle.html",
  "/docs/decisions.html",
  "/docs/contributing.html",
  "/docs/glossary.html",
];

async function seriousViolations(page, url, theme) {
  if (theme === "dark") await page.emulateMedia({ colorScheme: "dark" });
  await page.goto(url);
  const results = await new AxeBuilder({ page })
    .withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa", "wcag22aa"])
    .analyze();

  return results.violations
    .filter((violation) => ["serious", "critical"].includes(violation.impact))
    .map((violation) => ({
      id: violation.id,
      impact: violation.impact,
      help: violation.help,
      nodes: violation.nodes.map((node) => node.target),
    }));
}

for (const url of pages) {
  test(`axe light: ${url} has no serious or critical violations`, async ({ page }) => {
    const report = await seriousViolations(page, url, "light");
    expect(report, JSON.stringify(report, null, 2)).toEqual([]);
  });
}

for (const url of pages) {
  test(`axe dark: ${url} has no serious or critical violations`, async ({ page }) => {
    const report = await seriousViolations(page, url, "dark");
    expect(report, JSON.stringify(report, null, 2)).toEqual([]);
  });
}
