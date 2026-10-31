const STORAGE_KEY = "jx-theme";
const root = document.documentElement;
const button = document.querySelector(".theme-toggle");
const media = window.matchMedia("(prefers-color-scheme: dark)");

function readChoice() {
  try {
    const value = localStorage.getItem(STORAGE_KEY);
    return value === "light" || value === "dark" ? value : null;
  } catch {
    return null;
  }
}

function currentTheme() {
  const chosen = root.dataset.theme;
  if (chosen === "light" || chosen === "dark") return chosen;
  return media.matches ? "dark" : "light";
}

function saveChoice(value) {
  try {
    localStorage.setItem(STORAGE_KEY, value);
  } catch {
    /* The choice still applies for this page. */
  }
}

function syncButton() {
  if (!button) return;
  const dark = currentTheme() === "dark";
  button.setAttribute("aria-pressed", String(dark));
  button.setAttribute("aria-label", dark ? "Switch to light theme" : "Switch to dark theme");
  button.textContent = dark ? "Light theme" : "Dark theme";
}

if (button) {
  syncButton();

  button.addEventListener("click", () => {
    const next = currentTheme() === "dark" ? "light" : "dark";
    root.dataset.theme = next;
    saveChoice(next);
    syncButton();
  });

  media.addEventListener("change", () => {
    if (!readChoice()) syncButton();
  });
}
