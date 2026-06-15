// Detached settings window. Loads settings.html in its own native window
// (opened from Rust on Cmd+, or the gear button). Talks to the same Tauri
// commands as the main window; nothing here is shown on the main window, so no
// cross-window sync is needed.
import "@fontsource-variable/bricolage-grotesque";
import "@fontsource-variable/hanken-grotesk";
import "@fontsource-variable/jetbrains-mono";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { getCurrentWindow } from "@tauri-apps/api/window";

type SettingsView = {
  save_dir: string | null;
  default_save_dir: string;
  has_api_key: boolean;
};

const saveDirEl = document.querySelector<HTMLSpanElement>("#save-dir")!;
const chooseDirBtn = document.querySelector<HTMLButtonElement>("#choose-dir")!;
const resetDirBtn = document.querySelector<HTMLButtonElement>("#reset-dir")!;
const apiKeyEl = document.querySelector<HTMLInputElement>("#api-key")!;
const saveKeyBtn = document.querySelector<HTMLButtonElement>("#save-key")!;
const keyStatusEl = document.querySelector<HTMLSpanElement>("#key-status")!;
const statusEl = document.querySelector<HTMLParagraphElement>("#settings-status")!;

async function refresh() {
  try {
    const s = await invoke<SettingsView>("get_settings");
    saveDirEl.textContent = s.save_dir ?? `${s.default_save_dir}  (default)`;
    keyStatusEl.textContent = s.has_api_key ? "A key is saved." : "No key saved.";
    apiKeyEl.placeholder = s.has_api_key ? "•••••••• (saved)" : "Paste key…";
  } catch (err) {
    statusEl.textContent = `Error loading settings: ${err}`;
  }
}

chooseDirBtn.addEventListener("click", async () => {
  try {
    const picked = await open({ directory: true, multiple: false });
    if (typeof picked === "string") {
      await invoke("set_save_dir", { path: picked });
      await refresh();
    }
  } catch (err) {
    statusEl.textContent = `Error choosing folder: ${err}`;
  }
});

resetDirBtn.addEventListener("click", async () => {
  try {
    await invoke("set_save_dir", { path: null });
    await refresh();
  } catch (err) {
    statusEl.textContent = `Error resetting folder: ${err}`;
  }
});

saveKeyBtn.addEventListener("click", async () => {
  // An empty/whitespace key clears the stored credential rather than saving one.
  const cleared = apiKeyEl.value.trim() === "";
  try {
    await invoke("set_api_key", { key: apiKeyEl.value });
    apiKeyEl.value = "";
    await refresh();
    statusEl.textContent = cleared ? "Key cleared." : "Key saved.";
  } catch (err) {
    statusEl.textContent = `Error saving key: ${err}`;
  }
});

// Esc closes the window, matching the old in-app panel.
window.addEventListener("keydown", (e) => {
  if (e.key === "Escape") void getCurrentWindow().close();
});

refresh();
