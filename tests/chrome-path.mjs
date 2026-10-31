import { chromium } from "@playwright/test";

const browser = await chromium.launch({ channel: "chromium" });
console.log(browser.browserType().executablePath());
await browser.close();
