const buttons = document.querySelectorAll("[data-copy]");

for (const button of buttons) {
  const targetId = button.getAttribute("data-copy");
  const target = targetId ? document.getElementById(targetId) : null;
  if (!target) continue;

  const status = document.querySelector(`[data-copy-status="${targetId}"]`);

  function announce(message) {
    if (status) status.textContent = message;
  }

  function fallback(text) {
    const area = document.createElement("textarea");
    area.value = text;
    area.setAttribute("readonly", "");
    area.className = "visually-hidden";
    document.body.append(area);
    area.select();

    let copied = false;
    try {
      copied = document.execCommand("copy");
    } catch {
      copied = false;
    }
    area.remove();
    return copied;
  }

  button.addEventListener("click", async () => {
    const text = target.textContent.trim();

    try {
      await navigator.clipboard.writeText(text);
      announce("Copied to the clipboard.");
    } catch {
      if (fallback(text)) announce("Copied to the clipboard.");
      else announce("Copy failed. Select the text and copy it manually.");
    }
  });
}
