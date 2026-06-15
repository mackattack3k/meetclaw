# MeetClaw Agent — design (issue #34)

Status: **signed off**. Implementation in progress (PR1).

Let the assistant go beyond suggesting questions and **take actions** during a
meeting — run CLI commands and search the web — gated by Claude Code-style
approval (per-call / allow-all-for-a-tool / auto mode), and steered by a
user-editable `MEETCLAW.md` instruction doc.

## Decisions (locked)

| Area | Decision |
| --- | --- |
| Brain | **Gemini function-calling** (reuse existing key + `analyze.rs` infra) |
| Trigger | **Proactive** ("I think I should do X") **+ a manual ask box** |
| v1 tools | `run_command` (CLI) and `web_search` |
| Approval | per-call · allow-all-for-tool (allow-rules) · auto mode |
| Config | `MEETCLAW.md`-style editable instruction doc in settings |

## How the Gemini loop works

Verified against the function-calling docs. One agent "run" is a loop:

1. Build `contents` from: system instruction (persona + `MEETCLAW.md` + safety
   rules), meeting context (transcript + notes), and the trigger (a manual ask,
   or the proactive nudge). Attach `tools: [{ functionDeclarations: [...] }]`
   with `toolConfig.functionCallingConfig.mode = "AUTO"`.
2. `POST …:generateContent`.
3. If the response has **`functionCall`** part(s) — `{ id, name, args }` — each
   is a **proposed action** ("I think I should do X"). The loop pauses here for
   approval.
4. On approval, execute the tool, then append a `model` turn (the `functionCall`)
   and a `user` turn with a **`functionResponse`** part carrying the matching
   `id` and the (truncated) result. Go to 2.
5. When the model returns plain text instead of a call, that text is the final
   agent message. End the run.

A hard **step cap** (e.g. 8 tool calls/run) prevents runaways.

### Where it pauses for approval (the hard part)

The loop runs on a dedicated **agent thread** (matching the codebase's
thread + blocking-`reqwest` style). When a call needs approval it:

- emits `agent-proposal` to the UI, then
- **blocks on an `mpsc::Receiver<Decision>`** until the frontend's
  `agent_decision` command sends the user's choice (or `agent_stop` cancels).

This keeps the whole conversation state (`contents`) owned by the thread — no
need to serialize/rehydrate it across round-trips. Auto-allowed and
rule-allowed calls skip the wait and run immediately.

## Tools (v1)

### `run_command`
```
run_command(command: string, rationale: string)  ->  { exit_code, stdout, stderr }
```
- `rationale` is required so the proposal card can show *why*.
- Executed via the user's shell (`$SHELL -lc <command>`), **default-deny** until
  approved.
- Guardrails: configurable working directory (default: the meetings/workspace
  dir), a wall-clock **timeout** (default 30s, killed on overrun), and output
  **truncation** (e.g. 10 KB of combined stdout/stderr) before it goes back to
  the model.
- Runs with the user's own privileges — see Security.

### `web_search`
```
web_search(query: string)  ->  { summary, sources: [{title, url}] }
```
- Implemented as a **separate Gemini call with `google_search` grounding** —
  reuses the existing key, **no new API dependency**. Returns a short grounded
  summary plus source links.
- Read-only, low-risk; can be auto-allowed by default rule if the user wants.
- Alternative considered: a dedicated search API (Brave/Tavily). Rejected for v1
  to avoid another key. Easy to swap behind the same tool later.

## Approval model

Each proposal can be resolved three ways (mirrors Claude Code):

1. **Allow once** — run this one call.
2. **Always allow** — persist an **allow-rule**. Scope is either the whole tool
   (`web_search`) or a command prefix (`run_command: gh *`, `npm test`, …),
   matched by leading tokens.
3. **Deny** — skip; the loop is told the call was denied and continues.

Plus a global **Auto mode** toggle: when on, every call runs without a prompt
(still logged + shown). **Off by default.**

Resolution order per call: auto mode → matching allow-rule → otherwise prompt.

## `MEETCLAW.md` (agent steering)

- Plain-markdown instructions the user edits in Settings — persona, what to do /
  avoid, project context, preferred tools. Injected into the system
  instruction each run.
- Stored as a file in the app config dir (`agent.md` next to `settings.json`);
  a multi-line doc fits a file better than a JSON string.
- **Security note:** natural-language text in `MEETCLAW.md` *steers* the agent
  but is **never** parsed to grant permissions. Allow-rules and auto mode are
  structured settings (in `settings.json`), the only things that bypass a
  prompt. This avoids prompt-injection-via-config escalating privileges.

## Rust ↔ UI protocol

**Commands (UI → Rust)**
- `agent_ask(text)` — start a run from a manual request.
- `agent_decision(call_id, decision)` — `decision` = `allow_once` |
  `allow_always{scope}` | `deny`.
- `agent_set_auto(enabled)` / `agent_stop()`.
- `get_agent_config()` / `set_agent_config(markdown)` — `MEETCLAW.md`.
- `get_allow_rules()` / `set_allow_rules(rules)`.

**Events (Rust → UI)**
- `agent-proposal { call_id, tool, args, rationale }`
- `agent-tool-result { call_id, status, output }`
- `agent-message { text }` — the agent's final/intermediate prose.
- `agent-status { text }` / `agent-error { text }` / `agent-finished`.

## Proactive trigger

Piggyback the existing post-transcript analysis cadence. After
`suggest_questions`, run a lightweight "should I act?" pass (same context +
tools, `AUTO` mode). A `functionCall` becomes a proposal; plain text means "do
nothing". Kept quiet via: throttling, dedupe against recent proposals, and a
prompt that says to propose actions **sparingly, only on clear intent**. Tuning
expected — this is the noisiest piece, so it ships last (see phasing).

## UI placement (to confirm during build)

Proposals, results, the run log, and the ask box need a home. Leading option:
**broaden the right "Suggested questions" panel into an "Assistant" panel** with
the questions list, an agent activity log (proposal cards + results), and the
ask box pinned at the bottom. Avoids adding a 4th column at the 1060px default
width. Proposal card = tool + exact command/query + rationale + [Allow once]
[Always allow ▾] [Deny], with an Auto-mode toggle and a Stop control in the
header. Final UX confirmed when we get there.

## Security (the main risk)

`run_command` executes arbitrary shell as the user. Mitigations in v1:
- **Default-deny**; nothing runs without an explicit allow (once / rule / auto).
- The **exact command is always shown** before running, and **output is shown**.
- **Auto mode off by default**; allow-rules are opt-in and scoped.
- **Allow-rules never auto-run a command with shell metacharacters** (`; & | ` `` ` ``
  `$ ( ) < > * ? { } [ ] \`, quotes, newlines). So `run_command:gh` can't be
  widened into `gh && rm -rf ~` — chained/redirected commands always fall through
  to an explicit prompt where the user sees the full command. (Auto mode, by the
  user's choice, still bypasses prompts entirely.)
- Transcript and notes are framed as **untrusted data, not instructions**, in the
  system prompt (defends the proactive path in PR2 against meeting-injected
  "run rm -rf" lines).
- Timeout + output truncation; configurable working directory.
- Config text can't grant permissions (see `MEETCLAW.md` note).

**Accepted tradeoff:** Auto mode runs `run_command` without a prompt — requested
explicitly (see Decisions). It's off by default and the command/output are always
logged. Users who want it should pair it with a scoped workspace.

Explicitly **out of scope for v1** (follow-ups): OS-level sandboxing /
containerization (e.g. `sandbox-exec`), network egress control, secret redaction
in command output, and an argv-level allowlist classifier that rejects flag-form
exec smuggling (`git -c core.sshCommand=…`, `find -exec`, `tar
--checkpoint-action=exec=…`).

## Phasing (signed off)

1. **PR1 — agent core**: agent thread + Gemini loop + **`run_command` and
   `web_search`** + approval protocol + allow-rules + auto mode + `MEETCLAW.md`
   + the ask box & Assistant panel.
2. **PR2 — proactive trigger** + noise tuning.

## Resolved decisions

1. **UI:** broaden the right "Suggested questions" panel into an **"Assistant"**
   panel (questions + agent activity log + ask box). No 4th column.
2. **`run_command` working directory:** a configured **agent workspace** —
   default `…/<app data dir>/agent-workspace` (created on first use), changeable
   in Settings.
3. **`web_search`:** ships with a **default allow-rule** (auto-allowed,
   read-only). `run_command` still always prompts unless a rule/auto applies.
4. **Phasing:** `web_search` folded into **PR1**; proactive trigger is PR2.
