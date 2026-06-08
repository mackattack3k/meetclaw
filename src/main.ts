import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

let listening = false;

const toggleBtn = document.querySelector<HTMLButtonElement>("#toggle-btn")!;
const statusText = document.querySelector<HTMLParagraphElement>("#status-text")!;
const statusDot = document.querySelector<HTMLSpanElement>("#status-dot")!;
const transcriptEl = document.querySelector<HTMLElement>("#transcript")!;
const suggestionsEl = document.querySelector<HTMLElement>("#suggestions")!;
const modelSelect = document.querySelector<HTMLSelectElement>("#model-select")!;
const notesEl = document.querySelector<HTMLTextAreaElement>("#notes")!;

function setListening(on: boolean) {
  listening = on;
  toggleBtn.textContent = on ? "Stop listening" : "Start listening";
  statusDot.classList.toggle("live", on);
  statusText.textContent = on
    ? "Listening… transcribing every few seconds."
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

// Keep the backend's selected model in sync with the dropdown.
async function applyModel() {
  try {
    await invoke("set_model", { model: modelSelect.value });
  } catch (err) {
    statusText.textContent = `Error setting model: ${err}`;
  }
}

modelSelect.addEventListener("change", applyModel);
applyModel(); // push the default on load

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
