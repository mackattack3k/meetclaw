import "@fontsource-variable/bricolage-grotesque";
import "@fontsource-variable/hanken-grotesk";
import "@fontsource-variable/jetbrains-mono";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";

let listening = false;

const toggleBtn = document.querySelector<HTMLButtonElement>("#toggle-btn")!;
const statusText = document.querySelector<HTMLParagraphElement>("#status-text")!;
const statusDot = document.querySelector<HTMLSpanElement>("#status-dot")!;
const transcriptEl = document.querySelector<HTMLElement>("#transcript")!;
const suggestionsEl = document.querySelector<HTMLElement>("#suggestions")!;
const modelSelect = document.querySelector<HTMLSelectElement>("#model-select")!;
const deviceSelect = document.querySelector<HTMLSelectElement>("#device-select")!;
const notesEl = document.querySelector<HTMLTextAreaElement>("#notes")!;
const titleInput = document.querySelector<HTMLInputElement>("#title-input")!;
const libraryBtn = document.querySelector<HTMLButtonElement>("#library-btn")!;
const newBtn = document.querySelector<HTMLButtonElement>("#new-btn")!;
const libraryEl = document.querySelector<HTMLElement>("#library")!;
const libraryListEl = document.querySelector<HTMLElement>("#library-list")!;
const libraryCloseEl = document.querySelector<HTMLButtonElement>("#library-close")!;
const settingsBtn = document.querySelector<HTMLButtonElement>("#settings-btn")!;
const settingsEl = document.querySelector<HTMLElement>("#settings")!;
const settingsCloseEl = document.querySelector<HTMLButtonElement>("#settings-close")!;
const activeModelEl = document.querySelector<HTMLSpanElement>("#active-model")!;
const saveDirEl = document.querySelector<HTMLSpanElement>("#save-dir")!;
const chooseDirBtn = document.querySelector<HTMLButtonElement>("#choose-dir")!;
const resetDirBtn = document.querySelector<HTMLButtonElement>("#reset-dir")!;
const apiKeyEl = document.querySelector<HTMLInputElement>("#api-key")!;
const saveKeyBtn = document.querySelector<HTMLButtonElement>("#save-key")!;
const keyStatusEl = document.querySelector<HTMLSpanElement>("#key-status")!;

type SettingsView = {
  model: string;
  device: string | null;
  save_dir: string | null;
  default_save_dir: string;
  has_api_key: boolean;
};

type MeetingMeta = {
  id: string;
  title: string;
  created_at_ms: number;
  updated_at_ms: number;
};
type MeetingDetail = {
  id: string;
  title: string;
  created_at_ms: number;
  transcript: string;
  notes: string;
  suggestions: string[][];
};

function setListening(on: boolean) {
  listening = on;
  toggleBtn.textContent = on ? "Stop" : "Start";
  toggleBtn.classList.toggle("recording", on);
  statusDot.classList.toggle("live", on);
  statusText.textContent = on
    ? "Listening — transcribing on pauses."
    : "Stopped.";
}

function appendTranscript(text: string) {
  const placeholder = transcriptEl.querySelector(".placeholder");
  if (placeholder) placeholder.remove();

  const line = document.createElement("p");
  line.className = "line";
  line.textContent = text;
  transcriptEl.appendChild(line);
  transcriptEl.scrollTop = transcriptEl.scrollHeight;
}

// Replace the suggestions panel with the latest set of questions.
function renderSuggestions(questions: string[]) {
  suggestionsEl.replaceChildren();
  for (const q of questions) {
    const item = document.createElement("p");
    item.className = "suggestion";
    item.textContent = q;
    suggestionsEl.appendChild(item);
  }
}

function noteInSuggestions(message: string) {
  const note = document.createElement("p");
  note.className = "placeholder";
  note.textContent = message;
  suggestionsEl.replaceChildren(note);
}

toggleBtn.addEventListener("click", async () => {
  try {
    if (listening) {
      await invoke("stop_listening");
      setListening(false);
    } else {
      setListening(true);
      await invoke("start_listening");
    }
  } catch (err) {
    statusText.textContent = `Error: ${err}`;
    setListening(false);
  }
});

// --- Settings (model, device, save location, API key) ---

function updateModelChip() {
  const opt = modelSelect.options[modelSelect.selectedIndex];
  activeModelEl.textContent = opt ? opt.text : modelSelect.value;
}

modelSelect.addEventListener("change", () => {
  invoke("set_model", { model: modelSelect.value }).catch((err) => {
    statusText.textContent = `Error setting model: ${err}`;
  });
  updateModelChip();
});

deviceSelect.addEventListener("change", () => {
  invoke("set_device", { device: deviceSelect.value }).catch((err) => {
    statusText.textContent = `Error setting device: ${err}`;
  });
});

// Populate the input-device dropdown (options only; selection synced later).
async function loadDevices() {
  try {
    const devices = await invoke<string[]>("list_devices");
    for (const name of devices) {
      const opt = document.createElement("option");
      opt.value = name;
      opt.textContent = name;
      deviceSelect.appendChild(opt);
    }
  } catch (err) {
    statusText.textContent = `Error listing devices: ${err}`;
  }
}

function showSaveDir(s: SettingsView) {
  saveDirEl.textContent = s.save_dir ?? `${s.default_save_dir}  (default)`;
}

function showKeyStatus(has: boolean) {
  keyStatusEl.textContent = has ? "A key is saved." : "No key saved.";
  apiKeyEl.placeholder = has ? "•••••••• (saved)" : "Paste key…";
}

async function refreshSettings() {
  try {
    const s = await invoke<SettingsView>("get_settings");
    modelSelect.value = s.model;
    deviceSelect.value = s.device ?? "";
    showSaveDir(s);
    showKeyStatus(s.has_api_key);
    updateModelChip();
  } catch (err) {
    statusText.textContent = `Error loading settings: ${err}`;
  }
}

function hideSettings() {
  settingsEl.classList.add("hidden");
}
settingsBtn.addEventListener("click", () => {
  refreshSettings();
  settingsEl.classList.remove("hidden");
});
settingsCloseEl.addEventListener("click", hideSettings);
settingsEl.addEventListener("click", (e) => {
  if (e.target === settingsEl) hideSettings();
});

chooseDirBtn.addEventListener("click", async () => {
  try {
    const picked = await open({ directory: true, multiple: false });
    if (typeof picked === "string") {
      await invoke("set_save_dir", { path: picked });
      await refreshSettings();
    }
  } catch (err) {
    statusText.textContent = `Error choosing folder: ${err}`;
  }
});

resetDirBtn.addEventListener("click", async () => {
  try {
    await invoke("set_save_dir", { path: null });
    await refreshSettings();
  } catch (err) {
    statusText.textContent = `Error resetting folder: ${err}`;
  }
});

saveKeyBtn.addEventListener("click", async () => {
  try {
    await invoke("set_api_key", { key: apiKeyEl.value });
    apiKeyEl.value = "";
    await refreshSettings();
    keyStatusEl.textContent = "Saved.";
  } catch (err) {
    statusText.textContent = `Error saving key: ${err}`;
  }
});

// Initial load: populate devices, then sync saved settings into the controls.
(async () => {
  await loadDevices();
  await refreshSettings();
})();

// Sync the user's notes to the backend, debounced so we don't spam on each keystroke.
let notesTimer: number | undefined;
notesEl.addEventListener("input", () => {
  window.clearTimeout(notesTimer);
  notesTimer = window.setTimeout(() => {
    invoke("set_notes", { notes: notesEl.value }).catch((err) => {
      statusText.textContent = `Error saving notes: ${err}`;
    });
  }, 400);
});

// --- Meeting title, library, and persistence ---

let titleTimer: number | undefined;
titleInput.addEventListener("input", () => {
  window.clearTimeout(titleTimer);
  titleTimer = window.setTimeout(() => {
    invoke("set_title", { title: titleInput.value }).catch((err) => {
      statusText.textContent = `Error saving title: ${err}`;
    });
  }, 400);
});

function resetPanes() {
  titleInput.value = "";
  notesEl.value = "";
  transcriptEl.replaceChildren(makePlaceholder("Transcript will appear here…"));
  noteInSuggestions("Questions will appear here…");
}

function makePlaceholder(text: string): HTMLElement {
  const p = document.createElement("p");
  p.className = "placeholder";
  p.textContent = text;
  return p;
}

newBtn.addEventListener("click", async () => {
  try {
    await invoke("new_meeting");
    resetPanes();
  } catch (err) {
    statusText.textContent = `Error creating meeting: ${err}`;
  }
});

function showLibrary() {
  libraryEl.classList.remove("hidden");
}
function hideLibrary() {
  libraryEl.classList.add("hidden");
}

libraryCloseEl.addEventListener("click", hideLibrary);
libraryEl.addEventListener("click", (e) => {
  if (e.target === libraryEl) hideLibrary();
});

libraryBtn.addEventListener("click", async () => {
  try {
    const meetings = await invoke<MeetingMeta[]>("list_meetings");
    renderLibrary(meetings);
    showLibrary();
  } catch (err) {
    statusText.textContent = `Error loading library: ${err}`;
  }
});

function renderLibrary(meetings: MeetingMeta[]) {
  libraryListEl.replaceChildren();
  if (meetings.length === 0) {
    libraryListEl.appendChild(makePlaceholder("No saved meetings yet."));
    return;
  }
  for (const m of meetings) {
    const row = document.createElement("div");
    row.className = "library-row";

    const info = document.createElement("button");
    info.className = "library-open";
    const date = new Date(m.created_at_ms).toLocaleString();
    const title = document.createElement("span");
    title.className = "library-title";
    title.textContent = m.title;
    const when = document.createElement("span");
    when.className = "library-date";
    when.textContent = date;
    info.append(title, when);
    info.addEventListener("click", () => openMeeting(m.id));

    const del = document.createElement("button");
    del.className = "btn btn-secondary library-delete";
    del.textContent = "Delete";
    del.addEventListener("click", async () => {
      try {
        await invoke("delete_meeting", { id: m.id });
        const meetings = await invoke<MeetingMeta[]>("list_meetings");
        renderLibrary(meetings);
      } catch (err) {
        statusText.textContent = `Error deleting meeting: ${err}`;
      }
    });

    row.append(info, del);
    libraryListEl.appendChild(row);
  }
}

async function openMeeting(id: string) {
  try {
    const d = await invoke<MeetingDetail>("load_meeting", { id });
    titleInput.value = d.title;
    notesEl.value = d.notes;

    transcriptEl.replaceChildren();
    const lines = d.transcript.split("\n").filter((l) => l.trim().length > 0);
    if (lines.length === 0) {
      transcriptEl.appendChild(makePlaceholder("Transcript will appear here…"));
    } else {
      for (const line of lines) appendTranscript(line);
    }

    if (d.suggestions.length > 0) {
      renderSuggestions(d.suggestions[d.suggestions.length - 1]);
    } else {
      noteInSuggestions("Questions will appear here…");
    }

    hideLibrary();
    statusText.textContent = "Loaded meeting. Press start to keep recording.";
  } catch (err) {
    statusText.textContent = `Error opening meeting: ${err}`;
  }
}

// Backend events
listen<{ text: string }>("transcript", (event) => {
  appendTranscript(event.payload.text);
});

listen("listening-started", () => setListening(true));
listen("listening-stopped", () => setListening(false));

listen<string>("transcribe-error", (event) => {
  statusText.textContent = `Error: ${event.payload}`;
  setListening(false);
});

// Phase 2: Claude question suggestions
listen<{ questions: string[] }>("suggestions", (event) => {
  renderSuggestions(event.payload.questions);
});

listen<string>("analysis-disabled", (event) => {
  noteInSuggestions(event.payload);
});

listen<string>("analysis-error", (event) => {
  statusText.textContent = `Analysis error: ${event.payload}`;
});

// Auto-generated meeting title (on stop, if still untitled). Don't clobber a
// title the user has typed in the meantime.
listen<string>("title-updated", (event) => {
  if (titleInput.value.trim() === "") {
    titleInput.value = event.payload;
  }
});
