// Copy buttons, and the landing page's headline, typed and then completed like the wordmark.

document.addEventListener("click", async (event) => {
  const button = event.target.closest("[data-cp-copy], [data-cp-copy-page]");
  if (!button) return;
  const text = button.dataset.cpCopy ?? button.parentElement.querySelector(".cp-source")?.value ?? "";
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    return;
  }
  const label = button.querySelector("span:last-child");
  if (!label) return;
  const before = label.textContent;
  label.textContent = "copied";
  setTimeout(() => (label.textContent = before), 1400);
});

let stopTyping = null;

function startTyping() {
  stopTyping?.();
  stopTyping = null;
  const typed = document.querySelector(".cp-typed[data-cp-claims]");
  if (typed && !matchMedia("(prefers-reduced-motion: reduce)").matches) stopTyping = type(typed);
}

if (window.document$) document$.subscribe(startTyping);
else document.addEventListener("DOMContentLoaded", startTyping);

function type(root) {
  const claims = JSON.parse(root.dataset.cpClaims);
  const head = root.querySelector(".cp-typed__head");
  const tail = root.querySelector(".cp-typed__tail");
  const timers = [];
  const later = (ms) => new Promise((resolve) => timers.push(setTimeout(resolve, ms)));
  let live = true;

  async function run() {
    for (let n = 1; live; n = (n + 1) % claims.length) {
      await later(2600);
      if (!live) return;
      // Accept the suggestion, clear the line, then type the next claim and suggest its rest.
      head.textContent += tail.textContent;
      tail.textContent = "";
      root.classList.add("is-accepted");
      await later(900);
      for (let i = head.textContent.length; i >= 0 && live; i -= 2) {
        head.textContent = head.textContent.slice(0, i);
        await later(14);
      }
      root.classList.remove("is-accepted");
      const [prefix, rest] = claims[n];
      for (let i = 1; i <= prefix.length && live; i++) {
        head.textContent = prefix.slice(0, i);
        await later(48 + Math.random() * 40);
      }
      await later(160);
      tail.textContent = rest;
    }
  }

  run();
  return () => {
    live = false;
    timers.forEach(clearTimeout);
  };
}
