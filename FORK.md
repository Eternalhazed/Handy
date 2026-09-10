# Fork notes — OpenRouter transcription

This checkout is a **local fork** of [Handy](https://github.com/cjpais/Handy) that adds
OpenRouter's hosted speech-to-text as a second transcription backend, alongside the
local models.

It is a fork on purpose: upstream does not accept remote STT providers
([cjpais/Handy#886](https://github.com/cjpais/Handy/pull/886) — "This is a local only
transcription app and it will stay that way"). Do not open upstream PRs for this work.

Everything here lives on the `openrouter-stt` branch. `main` stays a pristine copy of
upstream so merges stay boring.

## What the fork adds

- **Settings**: `transcription_provider` (`local` | `openrouter`) and
  `openrouter_transcription_model`. The local `selected_model` is untouched, so
  switching back restores the previous local choice. No settings-schema bump —
  the new fields are additive.
- **Client** (`src-tauri/src/openrouter_stt.rs`): keyless catalog discovery
  (`GET /api/v1/models?output_modalities=transcription`, filtered to audio→transcription),
  upload to `POST /api/v1/audio/transcriptions` as JSON + base64 16 kHz mono WAV.
  Fixed URLs, one total budget per request (90 s), and retries **only** when the
  provider did not process the upload (no response, 429, 5xx gateway) — see the
  `UploadError::retryable` docs. A rejected or already-transcribed request is never
  repeated, so a retry cannot bill twice.
- **Dispatch** (`actions.rs::transcribe_audio`): one async entry point shared by
  dictation, history retry and the headless CLI. The cloud branch never loads the
  local engine; cleanup (custom words, fillers, normalization, Chinese variant
  conversion) runs on cloud output exactly as it does locally.
- **Switching safety**: an operation gate on `TranscriptionManager` serializes
  record → transcribe → output against provider/model changes and history retries.
- **UI**: one OpenRouter tile on the Models page (searchable picker over the live
  catalog, shared API key, Use), one row in the footer model switcher, one tray item,
  cloud state in General settings, and onboarding support without any local download.
- `src/bindings.ts` is generated — run a debug build instead of editing it by hand.

## The key is shared with post-processing

The OpenRouter API key is the one already used for post-processing
(`post_process_api_keys.openrouter`). Choosing an STT model never changes the
post-processing provider, model, prompt or enable flag, and vice versa.

## Updating from upstream

```powershell
cd C:\tmp\handy-src
git fetch origin                    # origin = https://github.com/cjpais/Handy
git switch openrouter-stt
git merge origin/main               # resolve conflicts, then:
bun install                         # only if package.json/lock changed
cd src-tauri; cargo test --lib      # full local suite
cd ..; bun run build; bun run lint
bun run tauri build                 # the version you actually install
```

Prefer `git merge` over `git rebase` here: the branch is used daily, and merges keep
resolved conflicts visible instead of rewriting history.

### Conflict hotspots

These upstream files carry fork changes, so expect conflicts here first:

| Area | Files |
| --- | --- |
| Settings | `src-tauri/src/settings.rs` (enum, 2 fields, defaults, `openrouter_transcription_ready`) |
| Dispatch | `src-tauri/src/actions.rs` (`transcribe_audio`, start/stop wiring, `process_transcription_output` signature) |
| Engine/state | `src-tauri/src/managers/transcription.rs` (operation gate, two `pub(crate)` helpers) |
| Commands | `src-tauri/src/commands/{models,history}.rs` |
| App shell | `src-tauri/src/lib.rs` (module list, command registry, headless CLI, tray handler) |
| Tray | `src-tauri/src/tray.rs` (`MenuInputs` + model submenu) |
| Reused helpers | `src-tauri/src/llm_client.rs`, `managers/model.rs`, `audio_toolkit/**` |
| Backend config | `src-tauri/Cargo.toml` (`base64`, `tokio` features) |
| Frontend | `ModelsSettings.tsx`, `ModelSelector.tsx`, `ModelDropdown.tsx`, `ModelSettingsCard.tsx`, `AdvancedSettings.tsx`, `Onboarding.tsx`, `Footer.tsx`, `App.tsx`, `i18n/locales/en/translation.json` |
| Generated | `src/bindings.ts` — regenerate with a debug run, never hand-merge |

### Checklist after a merge

1. `cargo test --lib` (278+ tests, incl. 14 `openrouter_stt_*`).
2. `bun run build` and `bun run lint`.
3. Launch the built app: Models page tile renders and lists the live catalog; footer
   shows `OpenRouter · <model>`; tray submenu has one OpenRouter entry.
4. One real dictation through OpenRouter (paste + history row).

## Keeping updates out of the way of the updater

The bundled auto-updater points at upstream releases. If it installs one, this feature
disappears. Keep **Settings → Advanced → "Check for updates"** disabled in the fork;
updates arrive through the merge workflow above instead.
