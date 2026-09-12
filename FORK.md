# Fork notes — OpenRouter transcription

This [public fork](https://github.com/Eternalhazed/Handy) of
[Handy](https://github.com/cjpais/Handy) adds OpenRouter's hosted speech-to-text as
a second transcription backend, alongside the local models. It is unofficial;
upstream's MIT license and attribution are retained.

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
  Fixed URLs and a shared 90-second network deadline, starting after audio encoding.
  At most three attempts, with 400 ms / 1200 ms backoff, for connection-establishment
  failures and HTTP 429 only. All 5xx (including 503), ambiguous transport failures,
  and malformed success responses are final. This is not an exactly-once or
  no-double-billing guarantee; a manual History retry may resend processed audio.
- **Dispatch** (`actions.rs::transcribe_audio`): one async entry point shared by
  dictation, history retry and the headless CLI. The cloud branch never loads the
  local engine; cleanup (custom words, fillers, normalization, Chinese variant
  conversion) runs on cloud output exactly as it does locally.
- **Switching safety**: an operation gate on `TranscriptionManager` serializes
  record → transcribe → output against provider/model changes and history retries.
  A separate short settings lock serializes shared API-key saves with cloud
  activation, preventing that activation from overwriting a concurrent key edit.
  It does not lock an in-flight transcription or make all legacy settings writers
  transactional.
- **UI**: one OpenRouter tile on the Models page (searchable picker over the live
  catalog, shared API key, Use), one row in the footer model switcher, one tray item,
  cloud state in General settings, and onboarding support without any local download.
- `src/bindings.ts` is generated — run a debug build instead of editing it by hand.

## The key is shared with post-processing

The OpenRouter API key is the one already used for post-processing
(`post_process_api_keys.openrouter`). Choosing an STT model never changes the
post-processing provider, model, prompt or enable flag, and vice versa.

## Branch and review workflow

- `origin` remains `https://github.com/cjpais/Handy`; `fork` points at
  `https://github.com/Eternalhazed/Handy`.
- Keep `main` aligned with upstream. Develop and build on `openrouter-stt`.
- [Fork PR #1](https://github.com/Eternalhazed/Handy/pull/1) is the review record.
  Keep it unmerged while `main` is an upstream mirror; releases can target the
  feature branch without merging it.
- Do not publish personal settings, API keys, history, recordings, or portable
  `Data/` directories with an artifact.

## Updating from upstream

```powershell
cd C:\tmp\handy-src
git fetch origin                    # origin = https://github.com/cjpais/Handy
git switch openrouter-stt
git merge origin/main               # resolve conflicts, then:
bun install                         # only if package.json/lock changed
cd src-tauri; cargo test --lib      # full local suite
cd ..; bun run build; bun run lint
# Package locally using the unsigned Windows command below.
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

1. `cargo test --lib`, `cargo fmt --check`, and `cargo clippy --lib --tests`.
2. `bun run build`, `bun run lint`, and `bun run check:translations`.
3. Launch an isolated portable build: exercise the Models tile, focused keyboard
   search/Escape, footer source switch, local-to-cloud unload, restart/rescan
   persistence, missing-key rejection, and narrow-window layout.
4. Loopback tests cover upload encoding, response failures, retry counts, and
   cancellation. Never infer recognition quality from those tests. Any additional
   live dictation requires explicit approval of the model, audio, and call count.

## Building an unsigned Windows candidate

Keep the checked-in signing and updater configuration unchanged. Supply a
build-only override instead (PowerShell):

```powershell
$env:CMAKE = 'C:\Program Files\CMake\bin\cmake.exe'
$env:VULKAN_SDK = 'C:\VulkanSDK\1.4.357.0'
bun run tauri build --bundles nsis --config '{"bundle":{"createUpdaterArtifacts":false,"windows":{"signCommand":null}}}'
```

This produces `src-tauri/target/release/bundle/nsis/Handy_0.9.6_x64-setup.exe`.
It is unsigned and may trigger Windows SmartScreen. Building is not installation.
Record the source commit and SHA-256 before distributing it.

For isolated execution, place a sibling `portable` file containing exactly
`Handy Portable Mode` next to the test executable. Its data and Hugging Face cache
then live under sibling `Data/`. Do not copy a normal profile into this sandbox.
An ordinary `cargo build` debug executable uses `devUrl` and needs the Vite server;
`bun run tauri build --debug --no-bundle` or a release build embeds the frontend.

## Keeping updates out of the way of the updater

The bundled updater points at upstream releases. If it installs one, this feature
disappears. In Debug settings (toggle visibility with `Ctrl+Shift+D` on Windows),
disable **Check for Updates**. Do not manually install an upstream update from the
footer. Updates to this fork arrive through the merge/build workflow above.
