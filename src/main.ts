import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

let listening = false;

const toggleBtn = document.querySelector<HTMLButtonElement>("#toggle-btn")!;
const statusText = document.querySelector<HTMLParagraphElement>("#status-text")!;
const statusDot = document.querySelector<HTMLSpanElement>("#status-dot")!;
const transcriptEl = document.querySelector<HTMLElement>("#transcript")!;

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
