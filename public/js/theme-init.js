(() => {
  "use strict";

  const root = document.documentElement;
  root.dataset.js = "true";

  try {
    const saved = localStorage.getItem("jx-theme");
    if (saved === "light" || saved === "dark") root.dataset.theme = saved;
  } catch {
    /* Storage can be unavailable; the system preference still applies. */
  }
})();
