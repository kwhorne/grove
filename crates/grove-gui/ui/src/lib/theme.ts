// Theme handling shared with the Conductor look: auto / light / dark.
// The dark palette is the default (no attribute); light sets data-theme="light".

export type Theme = "auto" | "light" | "dark";

const KEY = "grove-theme";

export function getTheme(): Theme {
  return (localStorage.getItem(KEY) as Theme) || "auto";
}

export function applyTheme(theme: Theme) {
  localStorage.setItem(KEY, theme);
  const root = document.documentElement;
  const wantLight =
    theme === "light" ||
    (theme === "auto" &&
      window.matchMedia("(prefers-color-scheme: light)").matches);
  if (wantLight) {
    root.setAttribute("data-theme", "light");
  } else {
    root.removeAttribute("data-theme");
  }
}

/** Apply the stored theme and keep `auto` in sync with the OS. */
export function initTheme() {
  applyTheme(getTheme());
  window
    .matchMedia("(prefers-color-scheme: light)")
    .addEventListener("change", () => {
      if (getTheme() === "auto") applyTheme("auto");
    });
}
