const figure = document.querySelector("[data-terminal]");

if (figure) {
  const staticPre = figure.querySelector(".terminal-static");
  const animated = figure.querySelector(".terminal-animated");
  const output = animated ? animated.querySelector("code") : null;
  const pauseButton = figure.querySelector("[data-terminal-pause]");
  const allButton = figure.querySelector("[data-terminal-all]");
  const reduce = window.matchMedia("(prefers-reduced-motion: reduce)");

  if (staticPre && output && pauseButton && allButton && !reduce.matches && "IntersectionObserver" in window) {
    const transcript = staticPre.textContent.trim().split("\n");
    const commands = transcript.filter((line) => line.startsWith("$ ")).map((line) => line.slice(2));
    const result = transcript.find((line) => !line.startsWith("$ ")) ?? "";

    let started = false;
    let paused = false;
    let stopped = false;
    let timer = 0;
    let resolveSleep = null;
    let cursor = null;

    function sleep(ms) {
      return new Promise((resolve) => {
        resolveSleep = resolve;
        timer = window.setTimeout(() => {
          timer = 0;
          resolveSleep = null;
          resolve();
        }, ms);
      });
    }

    function wake() {
      if (timer) window.clearTimeout(timer);
      timer = 0;
      if (resolveSleep) {
        const resolve = resolveSleep;
        resolveSleep = null;
        resolve();
      }
    }

    function waitWhilePaused() {
      if (!paused) return Promise.resolve();
      return new Promise((resolve) => {
        const check = () => {
          if (stopped || !paused) resolve();
          else window.setTimeout(check, 120);
        };
        check();
      });
    }

    function makeLine(kind) {
      const line = document.createElement("span");
      line.className = "terminal-line";

      const prompt = document.createElement("span");
      prompt.className = "terminal-prompt";
      prompt.textContent = "$ ";
      prompt.setAttribute("aria-hidden", "true");

      const text = document.createElement("span");
      text.className = kind;

      if (kind === "terminal-output") line.append(text);
      else line.append(prompt, text);

      output.append(line);
      return text;
    }

    function placeCursor(node) {
      if (!cursor) {
        cursor = document.createElement("span");
        cursor.className = "terminal-cursor";
        cursor.setAttribute("aria-hidden", "true");
      }
      node.after(cursor);
    }

    async function type(node, text) {
      for (const character of text) {
        await waitWhilePaused();
        if (stopped) return false;
        node.textContent += character;
        placeCursor(node);
        await sleep(40);
      }
      return !stopped;
    }

    function showAll() {
      if (stopped) return;
      stopped = true;
      paused = false;
      wake();
      figure.classList.remove("is-animating");
      staticPre.classList.remove("visually-hidden");
      animated.hidden = true;
      output.replaceChildren();
      pauseButton.hidden = true;
      allButton.hidden = true;
      cursor = null;
    }

    async function play() {
      if (started) return;
      started = true;

      output.replaceChildren();
      const firstLine = makeLine("terminal-command");
      const secondLine = makeLine("terminal-command");

      figure.classList.add("is-animating");
      animated.hidden = false;
      staticPre.classList.add("visually-hidden");
      pauseButton.hidden = false;
      allButton.hidden = false;

      placeCursor(firstLine);
      if (!(await type(firstLine, commands[0] ?? ""))) return;
      await sleep(400);
      if (stopped) return;
      await waitWhilePaused();
      if (stopped) return;

      placeCursor(secondLine);
      if (!(await type(secondLine, commands[1] ?? ""))) return;
      await sleep(600);
      if (stopped) return;
      await waitWhilePaused();
      if (stopped) return;

      const resultLine = makeLine("terminal-output");
      resultLine.textContent = result;
      if (cursor) cursor.remove();
      cursor = null;
      pauseButton.hidden = true;
      allButton.hidden = true;
    }

    pauseButton.addEventListener("click", () => {
      paused = !paused;
      pauseButton.textContent = paused ? "Resume" : "Pause";
      if (!paused) wake();
    });

    allButton.addEventListener("click", showAll);

    reduce.addEventListener("change", (event) => {
      if (event.matches) showAll();
    });

    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) {
          observer.disconnect();
          void play();
        }
      },
      { threshold: 0.35 },
    );

    observer.observe(figure);
  }
}
