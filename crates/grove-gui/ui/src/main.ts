import { mount } from "svelte";
import "@fontsource/jetbrains-mono/400.css";
import "@fontsource/jetbrains-mono/500.css";
import App from "./App.svelte";
import "./app.css";

const app = mount(App, {
  target: document.getElementById("app")!,
});

// Fade out the boot splash once the app has mounted.
const splash = document.getElementById("splash");
if (splash) {
  setTimeout(() => {
    splash.classList.add("hide");
    splash.addEventListener("transitionend", () => splash.remove(), { once: true });
  }, 350);
}

export default app;
