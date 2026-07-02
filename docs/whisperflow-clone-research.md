# Cloning Wispr Flow ("WhisperFlow") as a fully-local, cross-platform app

Deep-research report, 2026-07-02. Sources were fetched and claims adversarially verified (3-vote panel) unless marked otherwise. Verification votes shown as ✓ (3-0 confirmed) or (unverified — verification agents hit the account spend limit; claims are from fetched sources but were not independently re-checked).

---

## 1. What Wispr Flow actually is

Disambiguation: "WhisperFlow" almost always refers to **Wispr Flow** (wisprflow.ai), a commercial AI voice-dictation app. (There are also small open-source repos literally named whisper-flow; they are unrelated streaming-transcription libraries.)

Verified product facts (all from primary sources at wisprflow.ai):

- **System-wide dictation, not a transcriber app.** "Flow works everywhere: Notion, Gmail, Google Docs, WhatsApp, Cursor, anything with a text field." A clone must inject text into whatever app has focus. ✓ 3-0
- **AI cleanup of raw speech.** Automatically removes "um," "uh," and other fillers; infers punctuation from pauses and tone. So the product is STT **plus** a post-processing stage. ✓ 3-0
- **Four cleanup intensity levels** — None / Light (fillers + grammar) / Medium (clarity + conciseness) / High (rewrites for brevity and polish) — plus a separate "Polish" transform. This defines the LLM feature set to replicate. ✓ 3-0
- **Self-updating personal dictionary** (correcting a spelling auto-adds the word), voice-shortcut snippets, and per-app writing styles (styles English-only on desktop). ✓ 3-0
- **Hotkey / push-to-talk activation**, extensible to any non-primary mouse button; cancel/Enter shortcuts rebindable. ✓ 3-0
- **Cloud-based by default.** Transcription data (transcripts, audio, dictation history) can be stored on Wispr's servers; there's a Cloud Sync toggle and a Privacy Mode. ✓ 3-0
- Unverified but consistent across reviews: all STT/AI processing happens in the cloud with **no offline mode** (reportedly OpenAI/Meta infrastructure); default desktop hotkey is the **fn key**; 100+ languages; ships for **Mac, Windows, iOS — no Linux**.

Two takeaways for the clone: a fully-local version is a genuine differentiator (privacy + offline), and Linux support fills a real gap.

## 2. Target architecture

```
global hotkey (hold = push-to-talk, tap = toggle)
  → mic capture (16 kHz mono)
  → VAD (Silero) to trim silence
  → local STT engine (whisper.cpp / Parakeet)     ← NOT Ollama
  → local LLM cleanup via Ollama API (None/Light/Medium/High)
  → text injection into focused app (type or clipboard-paste)
```

This exact pipeline (hotkey hold → mic → whisper.cpp → llama.cpp → inject at cursor) has been demonstrated end-to-end in a Tauri 2.0/Rust build (dev.to write-up, unverified but corroborated by the Handy codebase).

### 2.1 Ollama cannot do the STT stage

As of mid-2026, **Ollama does not support audio input** — audio-capable models load but can't receive audio (open feature request, [ollama/ollama#11798](https://github.com/ollama/ollama/issues/11798)). So the architecture is necessarily **local STT engine + Ollama LLM for cleanup**, which still satisfies "a local model running on Ollama does what WhisperFlow does": the Wispr-Flow-specific intelligence (filler removal, grammar, tone, rewriting) is exactly the LLM half.

### 2.2 Local STT engine options

(Benchmark figures from fetched blog benchmarks; unverified individually but mutually consistent.)

| Engine | Speed | Resources | Notes |
|---|---|---|---|
| **whisper.cpp** | ~8× real-time (large-v3, CUDA on RTX 4070) | tiny model ~74 MB → large ~3 GB | Best overall balance; CPU + AMD/Intel/NVIDIA GPU + Metal; the embedding standard (used by Handy, OpenWhispr, VoiceTypr) |
| **faster-whisper** | ~12× real-time (large-v3 int8, RTX 4070); <0.5 s latency for large-v3-turbo on RTX 4090; ~2–3 s on a MacBook | large-v3: ~6 GB VRAM fp16, ~3 GB int8 | Fastest Whisper variant on NVIDIA; Python (CTranslate2) |
| **NVIDIA Parakeet TDT** (v3 / 1.1B) | RTFx >2000 on GPU; ~10× faster than Whisper **on CPU** | ~2 GB RAM (CPU), ~4 GB VRAM | ~8% WER, beats Whisper large-v3 on English; auto language detection; runs via sherpa-onnx or transcribe-rs; best default for CPU-only machines |
| **Vosk** | real-time on CPU | ~500 MB RAM | Lightweight but noticeably lower accuracy; fallback for very weak hardware |
| Voxtral (Mistral) | Realtime variant: 8.72% WER at 480 ms delay | — | Best multilingual WER (Mini Transcribe V2: 5.90% vs 7.40% for Whisper large-v3 on FLEURS); newer, less embedded tooling |

**Recommendation:** whisper.cpp (GGML/GGUF, Metal/CUDA/Vulkan) as the primary engine with **Parakeet V3 via sherpa-onnx/transcribe-rs** as the CPU-optimized option — exactly the dual-engine setup Handy ships. ✓ 3-0 (Handy claim)

### 2.3 Ollama LLM cleanup stage

- Call Ollama's local HTTP API (`/api/chat`, `stream: true`) with a system prompt per cleanup level; map the four Wispr levels to prompt variants. Personal dictionary and snippets are injected into the prompt (plus STT initial-prompt/hotwords where supported).
- Small instruct models are sufficient for filler-removal/grammar/formatting: **llama3.2:3b, qwen2.5:3b/7b, gemma3:4b** class models (recommendation — pick by testing on your hardware; research sources also used llama.cpp directly with 3–8B models for this job).
- Keep the LLM stage **optional and streaming**: "None" level bypasses it entirely, so dictation stays usable on machines that can't hold both models.
- VRAM budget: Whisper medium/turbo (~1.5–3 GB) + a 3–4B Q4 LLM (~2.5–3 GB) fits an 8 GB GPU; CPU-only machines should use Parakeet + a 3B model.

## 3. Cross-platform implementation

### 3.1 Text injection (the hard part)

- **[Enigo](https://github.com/enigo-rs/enigo)** (Rust, MIT): cross-platform input simulation; stable for text on **Windows (SendInput), macOS (CGEvent), Linux X11**; **Wayland and libei support are experimental**. ✓ 3-0 (both claims)
- macOS requires **Accessibility permission** (System Settings → Privacy & Security) for synthetic input; request/detect it at first run.
- **Wayland is the risk area**: xdotool cannot work (depends on X APIs), and Wayland's security model blocks cross-app input injection by design (unverified but well-established). Mitigations, in order of practicality:
  1. **Clipboard-paste injection** (set clipboard → synthesize Ctrl/Cmd+V → restore clipboard) — what Handy does via enigo + tauri-plugin-clipboard-manager; works on far more targets than per-char typing.
  2. `wtype` (virtual-keyboard protocol) / `ydotool` (uinput, needs a daemon/root) as Wayland fallbacks.
  3. Longer-term: libei / input-method protocols.
- Practical policy: **paste-injection as default everywhere, char-typing as an option** (some apps block programmatic paste; some terminals mangle it).

### 3.2 Global hotkeys

- Tauri v2 has an official global-shortcut plugin for Windows/Linux/macOS, with Rust-side press/release states usable for push-to-talk. ✓ (JS/Rust API claim 3-0) — **but** the adversarial panel refuted the stronger claim that it needs "no per-OS code": limitations exist (notably Wayland, and bare-modifier keys like fn are not standard shortcuts).
- Handy instead uses **rdev** (rustdesk fork) for low-level global key events — needed for hold-to-talk on arbitrary keys and better coverage. ✓ 3-0
- Wispr's default fn-key trigger is not portably capturable; use a configurable default like `Ctrl+Space` / `Alt+Space` and allow mouse-button bindings.

### 3.3 Audio capture + VAD

- **cpal** (Rust) for cross-platform mic capture, **rubato** for resampling to 16 kHz, **Silero VAD** (vad-rs) for silence filtering — Handy's proven stack. ✓ 3-0
- macOS also needs the Microphone permission prompt; Linux should target PipeWire (via cpal's ALSA/PipeWire backends).

### 3.4 Framework

**Tauri 2 (Rust core + web UI)** is the recommended frame: it is what Handy and VoiceTypr use, gives direct access to enigo/cpal/rdev/whisper-rs in-process, small binaries, and a tray + settings UI in React. Electron also demonstrably works (OpenWhispr) if you prefer a JS-first stack with whisper.cpp/sherpa-onnx as native modules.

## 4. Prior art (all verified against the repos, 2026-07-02)

| Project | License | Stack | What it proves / gaps |
|---|---|---|---|
| **[Handy](https://github.com/cjpais/Handy)** | MIT | Tauri, Rust: cpal + rdev + Silero VAD + rubato + enigo; whisper.cpp (transcribe-cpp) + Parakeet V3 (transcribe-rs) | ✓ Closest match: offline, cross-platform (Win/mac/Linux), push-to-talk → paste into focused app, ~5× real-time Parakeet on CPU. **Gap: no LLM cleanup stage** — exactly where your Ollama layer adds value |
| **[OpenWhispr](https://github.com/OpenWhispr/openwhispr)** | MIT | Electron 41, React 19, better-sqlite3; whisper.cpp + sherpa-onnx (Parakeet); optional cloud LLMs (BYOK) | ✓ Ships installers for all three OSes (.dmg/.exe/.AppImage/.deb/.rpm); validates local-STT + LLM-cleanup architecture; self-described "open-source alternative to WisprFlow." LLM cleanup is cloud-BYOK → your all-local Ollama version differentiates |
| **[VoiceTypr](https://github.com/moinulmoin/voicetypr)** | AGPL-3.0 (unverified) | Tauri, Rust + React (unverified) | Wispr Flow alternative, macOS + Windows only — no Linux (unverified: spend limit killed these checks). AGPL matters if you borrow code |

Strategy implication: **fork or heavily crib from Handy (MIT)** for the system plumbing and add the missing Wispr-differentiating layer — Ollama cleanup levels, personal dictionary, snippets, per-app styles.

## 5. Recommended base-functionality feature list (v1)

1. Tray app with settings UI (Tauri 2, React).
2. Configurable global hotkey: hold = push-to-talk, tap = toggle; mouse-button binding.
3. cpal mic capture → Silero VAD → whisper.cpp (default: `large-v3-turbo` GGUF on GPU, Parakeet V3 on CPU); model picker with download manager.
4. Ollama cleanup with the four Wispr levels (None/Light/Medium/High) + "Polish" transform; model configurable, default a 3–4B instruct model; graceful degradation to raw transcript if Ollama is down.
5. Personal dictionary (fed to both STT hotwords and the LLM prompt) + text snippets.
6. Injection: clipboard-paste with clipboard restore (default) or simulated typing (enigo); X11 native, Wayland via paste/wtype fallback.
7. Recording indicator overlay + dictation history (local SQLite).
8. Per-OS permissions onboarding (macOS mic + Accessibility; Wayland caveat messaging).

**Realistic latency expectations** (hold-to-talk, ~10 s utterance): STT 0.3–1.5 s on a modern GPU (faster-whisper turbo <0.5 s on high-end NVIDIA; 2–3 s on Apple-Silicon MacBooks; Parakeet CPU ≈ real-time ÷ 5–10), plus LLM cleanup ~0.3–1 s for a 3B model on GPU (stream it to hide latency). End-to-end ~1–3 s from key-release to text appearing is achievable; "None" cleanup level lands under a second on good hardware. Wispr-style *streaming-while-you-talk* injection is a v2 feature (requires chunked/streaming STT — Voxtral Realtime or whisper.cpp streaming — and complicates cleanup).

## 6. Verification caveats

- 16 claims confirmed 3-0 by adversarial verification; 1 refuted (the "Tauri plugin means zero per-OS hotkey code" overreach — the plugin exists and is cross-platform, but Wayland/fn-key limits are real).
- 8 claims (VoiceTypr details; Wispr cloud-only/no-offline; 100+ languages; fn default; Mac/Win/iOS-only) went **unverified** because the account's monthly Claude spend limit was hit mid-run — they come from fetched sources and are mutually consistent, but treat them as high-confidence-unverified.
- Benchmark numbers (RTF, WER, VRAM) are from 2026 blog benchmarks fetched during research, not independently reproduced; validate on your own hardware before locking the default model.

### Key sources

- https://wisprflow.ai/features, https://wisprflow.ai/whats-new (primary)
- https://github.com/cjpais/Handy, https://github.com/OpenWhispr/openwhispr, https://github.com/moinulmoin/voicetypr (primary)
- https://github.com/enigo-rs/enigo, https://v2.tauri.app/plugin/global-shortcut/ (primary)
- https://github.com/ollama/ollama/issues/11798 (primary — Ollama audio support)
- Benchmarks/reviews: northflank.com (STT benchmarks 2026), promptquorum.com, sinologic.net (Vosk vs Whisper), assemblyai.com, weesperneonflow.ai (Voxtral comparison), willowvoice.com, zapier.com/blog/wispr-flow
