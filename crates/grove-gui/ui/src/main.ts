import { mount } from "svelte";
import "@fontsource/jetbrains-mono/400.css";
import "@fontsource/jetbrains-mono/500.css";
import App from "./App.svelte";
import "./app.css";
import { initTheme } from "./lib/theme";

function hideSplash() {
  const splash = document.getElementById("splash");
  if (!splash) return;
  splash.classList.add("hide");
  splash.addEventListener("transitionend", () => splash.remove(), { once: true });
  // Hard fallback in case the transition event never fires.
  setTimeout(() => splash.remove(), 800);
}

let app: ReturnType<typeof mount> | undefined;
try {
  initTheme();
  app = mount(App, { target: document.getElementById("app")! });
} catch (err) {
  // Never leave a blank window: surface the error instead.
  const el = document.getElementById("app");
  if (el) {
    el.innerHTML = `<pre style="color:#f7768e;padding:24px;font-family:monospace;white-space:pre-wrap">Grove UI failed to start:\n\n${String(err)}</pre>`;
  }
  console.error(err);
}

// Remove the boot splash once the app has had a moment to render.
setTimeout(hideSplash, 300);

export default app;
