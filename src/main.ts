import "@fontsource-variable/bricolage-grotesque";
import "@fontsource-variable/hanken-grotesk";
import "@fontsource-variable/jetbrains-mono";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { confirm, open } from "@tauri-apps/plugin-dialog";

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
const langSelect = document.querySelector<HTMLSelectElement>("#lang-select")!;
const detectedLangEl = document.querySelector<HTMLSpanElement>("#detected-lang")!;
const sourceSeg = document.querySelector<HTMLElement>("#source-seg")!;
const saveDirEl = document.querySelector<HTMLSpanElement>("#save-dir")!;
const chooseDirBtn = document.querySelector<HTMLButtonElement>("#choose-dir")!;
const resetDirBtn = document.querySelector<HTMLButtonElement>("#reset-dir")!;
const apiKeyEl = document.querySelector<HTMLInputElement>("#api-key")!;
const saveKeyBtn = document.querySelector<HTMLButtonElement>("#save-key")!;
const keyStatusEl = document.querySelector<HTMLSpanElement>("#key-status")!;
const playerEl = document.querySelector<HTMLElement>("#player")!;
const playBtn = document.querySelector<HTMLButtonElement>("#play-btn")!;
const seekEl = document.querySelector<HTMLInputElement>("#seek")!;
const timeEl = document.querySelector<HTMLSpanElement>("#time")!;
const audioEl = document.querySelector<HTMLAudioElement>("#audio")!;

type SettingsView = {
  model: string;
  device: string | null;
  save_dir: string | null;
  default_save_dir: string;
  has_api_key: boolean;
  language: string;
  audio_source: string;
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
      clearPlayer();
      setListening(true);
      await invoke("start_listening");
    }
  } catch (err) {
    statusText.textContent = `Error: ${err}`;
    setListening(false);
  }
});

// --- Console controls (source, mic, language, model) ---

modelSelect.addEventListener("change", () => {
  invoke("set_model", { model: modelSelect.value }).catch((err) => {
    statusText.textContent = `Error setting model: ${err}`;
  });
});

deviceSelect.addEventListener("change", () => {
  invoke("set_device", { device: deviceSelect.value }).catch((err) => {
    statusText.textContent = `Error setting device: ${err}`;
  });
});

function cap(s: string): string {
  return s ? s[0].toUpperCase() + s.slice(1) : s;
}

// Tiny readout next to the Lang selector; shows the detected language on Auto.
function showDetectedLang(name?: string) {
  detectedLangEl.textContent =
    langSelect.value === "auto" && name ? ` · ${cap(name)}` : "";
}

langSelect.addEventListener("change", () => {
  invoke("set_language", { language: langSelect.value }).catch((err) => {
    statusText.textContent = `Error setting language: ${err}`;
  });
  showDetectedLang();
});

// Segmented audio-source control (Mic / System / Both).
const segButtons = Array.from(sourceSeg.querySelectorAll<HTMLButtonElement>(".seg"));
function setSourceActive(value: string) {
  for (const b of segButtons) b.classList.toggle("active", b.dataset.source === value);
}
for (const b of segButtons) {
  b.addEventListener("click", () => {
    const value = b.dataset.source!;
    setSourceActive(value);
    invoke("set_audio_source", { source: value }).catch((err) => {
      statusText.textContent = `Error setting audio source: ${err}`;
    });
  });
}

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
    langSelect.value = s.language;
    setSourceActive(s.audio_source);
    showSaveDir(s);
    showKeyStatus(s.has_api_key);
    showDetectedLang();
  } catch (err) {
    statusText.textContent = `Error loading settings: ${err}`;
  }
}

function hideSettings() {
  settingsEl.classList.add("hidden");
}
function openSettings() {
  refreshSettings();
  settingsEl.classList.remove("hidden");
}
settingsBtn.addEventListener("click", openSettings);
settingsCloseEl.addEventListener("click", hideSettings);

// Native macOS menu: Settings… (Cmd+,)
listen("menu:settings", openSettings);
// Esc closes the settings/library overlays.
window.addEventListener("keydown", (e) => {
  if (e.key === "Escape") {
    hideSettings();
    hideLibrary();
  }
});
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
  clearPlayer();
}

// --- Audio playback of a loaded meeting ---

let audioUrl: string | null = null;

function fmtTime(t: number): string {
  if (!isFinite(t) || t < 0) return "0:00";
  const m = Math.floor(t / 60);
  const s = Math.floor(t % 60);
  return `${m}:${s.toString().padStart(2, "0")}`;
}

function clearPlayer() {
  audioEl.pause();
  audioEl.removeAttribute("src");
  audioEl.load();
  if (audioUrl) {
    URL.revokeObjectURL(audioUrl);
    audioUrl = null;
  }
  playerEl.classList.add("hidden");
  playBtn.classList.remove("playing");
  seekEl.value = "0";
}

async function loadAudio(id: string) {
  try {
    const buf = await invoke<ArrayBuffer>("read_meeting_audio", { id });
    if (audioUrl) URL.revokeObjectURL(audioUrl);
    audioUrl = URL.createObjectURL(new Blob([buf], { type: "audio/wav" }));
    audioEl.src = audioUrl;
    seekEl.value = "0";
    timeEl.textContent = "0:00";
    playerEl.classList.remove("hidden");
  } catch {
    clearPlayer(); // meeting has no saved audio yet
  }
}

playBtn.addEventListener("click", () => {
  if (audioEl.paused) void audioEl.play();
  else audioEl.pause();
});
audioEl.addEventListener("play", () => playBtn.classList.add("playing"));
audioEl.addEventListener("pause", () => playBtn.classList.remove("playing"));
audioEl.addEventListener("ended", () => playBtn.classList.remove("playing"));
audioEl.addEventListener("loadedmetadata", () => {
  timeEl.textContent = fmtTime(audioEl.duration);
});
audioEl.addEventListener("timeupdate", () => {
  if (audioEl.duration > 0) {
    seekEl.value = String((audioEl.currentTime / audioEl.duration) * 1000);
  }
  timeEl.textContent = fmtTime(audioEl.currentTime);
});
seekEl.addEventListener("input", () => {
  if (audioEl.duration > 0) {
    audioEl.currentTime = (Number(seekEl.value) / 1000) * audioEl.duration;
  }
});

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
      const ok = await confirm(`Delete "${m.title}"? This can't be undone.`, {
        title: "Delete meeting",
        kind: "warning",
      });
      if (!ok) return;
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

    void loadAudio(id);
    hideLibrary();
    statusText.textContent = "Loaded meeting. Press play to listen, or start to keep recording.";
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

// Detected transcription language (meaningful when language = auto).
listen<string>("language-detected", (event) => {
  showDetectedLang(event.payload);
});

// System-audio helper status (permission errors, ready).
listen<string>("syscap-status", (event) => {
  statusText.textContent = event.payload;
});

// Two-tier transcript: the high-quality whole-file pass after Stop.
listen("transcript-finalizing", () => {
  statusText.textContent = "Refining transcript from the recording…";
});
listen<string>("transcript-finalized", (event) => {
  transcriptEl.replaceChildren();
  const sentences = event.payload.split(/(?<=[.?!])\s+/).filter((s) => s.trim());
  if (sentences.length === 0) {
    transcriptEl.appendChild(makePlaceholder("Transcript will appear here…"));
  } else {
    for (const s of sentences) appendTranscript(s);
  }
  statusText.textContent = "Transcript refined.";
});

// Auto-generated meeting title (on stop, if still untitled). Don't clobber a
// title the user has typed in the meantime.
listen<string>("title-updated", (event) => {
  if (titleInput.value.trim() === "") {
    titleInput.value = event.payload;
  }
});
