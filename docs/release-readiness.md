# Release readiness

This document describes the unreleased implementation as of 2026-10-08. Product/package name: Magpie; repository directory: `glint`. It is a release checklist, not a statement that every check has passed. Record the tested commit, hardware, OS, build type, logs and dated results before marking a release ready.

## Implemented scope

| Workflow            | Current behavior                                                                                                                                                           | Limit                                                                                                                                                                                                                   |
| ------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Search intent       | All fuses text and visual ranks; Text searches filename/OCR/notes/tags; Visual searches compatible image embeddings. Evidence stays attached to each result.               | Coverage and similarity thresholds affect visual recall. Scores are not probabilities.                                                                                                                                  |
| Filters and paging  | Folder/date/type controls plus query filters, including quoted `in:`, `tag:` and `collection:` values. Filters apply before vector scoring; bursts fold before pagination. | Offset pages are stable only while the index/request is unchanged. Refresh restarts at page one.                                                                                                                        |
| Index readiness     | OCR and visual work are interleaved with recent-image priority. UI shows processed counts, coverage, waiting reasons and failures.                                         | Downloaded/loaded models do not imply every image has been embedded.                                                                                                                                                    |
| Saved work context  | Named saved searches, notes, tags, manual collections and source URLs persist locally.                                                                                     | Collections are metadata, not copied-file directories; there is no automatic collection classifier.                                                                                                                     |
| Crop and similarity | Drop/paste an image, find similar to a result, or select a crop by dragging/percentage inputs.                                                                             | Requires visual search/model readiness. Search crops do not modify originals.                                                                                                                                           |
| Relevance feedback  | Not relevant hides a result for the exact normalized query; Restore result undoes it.                                                                                      | It does not fine-tune CLIP or suppress all related searches.                                                                                                                                                            |
| Capture             | Optional unpacked Chrome/Edge companion saves the visible webpage as a PNG with an optional source sidecar. Native import preserves existing user context.                 | Explicit click only; visible viewport only; no full-page scroll, history reading or background recording. No store publication.                                                                                         |
| Library control     | Watched folders, broader scanning, recursive exclusions, optional clipboard-image saving.                                                                                  | Exclusions can remove indexed context for those images; originals stay in their folders.                                                                                                                                |
| Recovery            | Pause/resume, Finish indexing now, retry failed files, rebuild, model repair, safe aggregate diagnostics and clear index.                                                  | Rebuild/retry use priority indexing; model repair may download again. Clear index removes image metadata/feedback and disables watched folders/clipboard saving; saved searches and remaining preferences are retained. |
| Export              | Selected original files are copied into a new export directory with a metadata/pin manifest.                                                                               | Failure can leave an incomplete new directory; inspect the error and retry. Capture sidecars/exports remain ordinary local files outside index clearing.                                                                |

The scanner supports PNG, JPG/JPEG, WebP, GIF, TIF/TIFF and BMP. GIF animation/TIFF multipage content is treated as a still image, not a searchable frame/page sequence. PDF, SVG, HEIC/HEIF, AVIF and RAW are outside the current format scope. Large decode limits and corrupt files can prevent OCR/embedding. The optional PaddleOCR recognizer is English; operating-system OCR depends on installed language support.

## Offline and privacy boundary

The desktop app performs indexing, OCR, embedding and search locally. Optional CLIP models require about 156 MB of initial downloads; Windows Sharper text requires about 13 MB. Downloads are pinned to revisions and SHA-256 checked. Once valid models are cached, inference runs offline. Repair can require network access again. No account, model API key, image-upload service or telemetry endpoint is required for local use.

Source links and detected links open websites only when the user chooses to open them. Capture source URLs can contain sensitive query parameters; the extension offers image-only capture. A source sidecar contains capture provenance and a title. Exported metadata can contain private notes and links. Clearing the index retains saved searches, exclusions and remaining preferences. Retained capture sidecars can import their source context again if their folders are watched later.

Diagnostic exports contain app version, OS/architecture, timestamp, aggregate image/processing/failure counts, model states, warning count and embedding version. They omit original paths, raw errors, OCR, notes and queries.

## Reproducible checks

Prerequisites: Node 24 (CI baseline), Rust 1.90 or newer, installed dependencies via `npm ci`, platform C++/SDK tools on Windows or Xcode command-line tools on macOS. Windows also needs WebView2 for the native UI. Dependency downloads need network access; model-dependent examples need already cached models. Use a separate test-image corpus and a new profile rather than the installed app's database.

Run from the repository root:

```powershell
npm ci
npm run typecheck
npm run lint
npm run test:renderer
npm run test:capture
cargo test --manifest-path src-tauri/Cargo.toml --lib
cargo check --manifest-path src-tauri/Cargo.toml --examples
npm run build:web
npm run build
```

| Check                    | What it proves                                                                                                                                          | What it does not prove                                                 |
| ------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------- |
| `test:renderer`          | Synthetic preview contract: modes, paging, metadata/filtering, exact-query feedback, controls, UTF-8 saved-name limits and clear-index settings parity. | Native commands, OCR, real-model relevance or OS integration.          |
| `test:capture`           | Mocked browser API sequencing, optional source context, page-change and download-failure handling.                                                      | A loaded or store-installed extension's permissions/download behavior. |
| Rust library tests       | Retrieval/indexing/metadata/download guards on deterministic fixtures.                                                                                  | Quality on a representative real corpus or packaged app UX.            |
| `cargo check --examples` | Example targets compile.                                                                                                                                | Any example has actually run.                                          |
| Web build                | Renderer bundle can be produced.                                                                                                                        | Native build, install/uninstall, signing or launch behavior.           |
| Native build             | Platform binary/bundle generation succeeds on the tested host.                                                                                          | Signing, notarization, store availability or operational updates.      |

Windows restricted shells can fail to launch esbuild with `spawn EPERM`. Record that as an execution-environment failure, then rerun with an authorized unrestricted build environment. A failed attempt is not a passing build.

### Real-model and native examples

Use the existing [search-quality guide](search-quality.md) for manifest schema, metrics and interpretation. Examples use local files and write reports; they do not upload images. Replace these example paths with your own test paths.

```powershell
# Actual CLIP inference and retrieval with reviewed labels; no installed library DB.
cargo run --manifest-path src-tauri/Cargo.toml --example search_eval -- 'C:\test\models' 'C:\test\corpus\manifest.json' 'C:\test\search-report.json'

# Fresh native scanner, OS OCR, thumbnails, embedding and retrieval on starter fixtures.
# The image folder must be the starter fixture corpus (001/002 dog photos and 007 invoice).
# Use a NEW profile directory; the example refuses existing profiles and copies pinned caches.
cargo run --manifest-path src-tauri/Cargo.toml --example engine_smoke -- 'C:\test\models' 'C:\test\starter\images' 'C:\test\new-smoke-profile' 'C:\test\native-report.json'

# Compare installed OS OCR and cached PaddleOCR on a permissioned screenshot folder.
cargo run --release --manifest-path src-tauri/Cargo.toml --example ocr_bench -- 'C:\test\screenshots' 'C:\test\models' invoice receipt
```

`search_eval` uses manifest-provided text, so its OCR-related hits do not measure OCR extraction. `engine_smoke` uses actual OS OCR and embedding, but the starter corpus is small. `ocr_bench` reports timings and needle hits; it is not a full character/word-accuracy benchmark. Use permissioned data, reviewed labels, distractors and held-out queries before making relevance claims. Include cold/warm latency, hardware and index coverage in reports.

For an indexing performance experiment, `engine_bench` can create a separate data directory and measure catalog/index/search timing. It may download enabled models when caches are absent and writes its profile. Do not point it at the installed profile.

```powershell
cargo run --release --manifest-path src-tauri/Cargo.toml --example engine_bench -- 'C:\test\separate-bench-profile' 'C:\test\screenshots' --semantic --active -- dog receipt
```

The older `scripts/e2e-tauri.mjs` is an invasive Windows helper with its own temporary profile/clipboard cleanup and an outdated invocation comment. Review its behavior before opting into it. The manual checklist below is the release gate; running the helper alone is not sufficient.

## Manual native OS QA

Perform each applicable check on Windows 10/11 and macOS 12+ (Intel and Apple Silicon build targets). Record untested hosts explicitly. Use disposable fixture images and a new app profile. Quit any installed instance yourself before testing a separate profile because single-instance handling can otherwise target the existing app. Never clear or reconfigure a real user's library for a smoke test.

For a Windows development profile:

```powershell
$env:MAGPIE_USER_DATA = 'C:\test\new-native-qa-profile'
npm run dev
# In that shell after the app exits:
Remove-Item Env:MAGPIE_USER_DATA
```

The environment variable selects a profile; it does not make destructive file actions safe. Use copies of fixture images. On macOS, set the same `MAGPIE_USER_DATA` variable in the launching shell and use a fresh directory. Test a release bundle as well as a development launch.

- [ ] Fresh launch/onboarding: add a fixture folder, decline optional features, relaunch and verify saved settings. Test missing/malformed settings recovery and a failed save without silent success.
- [ ] Index coverage: newly added files appear by name, OCR/visual counts progress together, recent images get attention, and model-loaded state is distinct from coverage. Verify download/loading/error states.
- [ ] Power and pause: normal heavy work waits where appropriate; Finish indexing now processes while active/on battery with resource copy visible. Pause stops priority work too; completion restores normal background behavior. Repeat rebuild/retry.
- [ ] Retrieval: test `dog` in All/Text/Visual against dog photos, cat distractors and screenshots containing only the word. Test phrases, typos, no matches, excluded folders, date filters and quoted collection/folder names. Verify evidence and backend page exhaustion.
- [ ] Stale responses: type/change modes faster than inference, then inject or trigger a search failure. Old results must not appear under the new query; retry must recover. Long libraries must page beyond former result ceilings without duplicate IDs.
- [ ] Context: save a note, tag, collection and valid source URL; relaunch and search/filter them. Reject executable/credential-bearing source URLs and over-limit values while preserving editable form text. Save/remove/recall a named query, including Unicode names.
- [ ] Relevance feedback: dismiss a result and restore it during the undo action. Dismiss again and relaunch to verify the exact-query dismissal persists. Other queries should remain unaffected.
- [ ] Image search: drop, paste, find similar and search a dragged/keyboard crop. Confirm original image decoding/CORS, crop bytes and transition to Visual. Switching to Text clears image context. Originals remain unchanged.
- [ ] OS actions: copy full/selected OCR text and images, paste into another native app, drag a file out, open/reveal, pin, hotkey summon/copy-latest, login behavior, theme, blur behavior and tray/menu actions. Unavailable text-copy actions must be disabled.
- [ ] Trash and recovery: use disposable copies to test delayed trash and Undo, including a burst and a locked/unavailable file. Verify the OS trash actually contains successfully moved files; report failures accurately.
- [ ] Library changes: add/remove/exclude nested folders and switch scope. Simulate temporarily unavailable folders and confirm incomplete scans do not silently erase indexed entries. Re-add a file or modify it and verify reindexing/version behavior.
- [ ] Recovery: trigger a corrupt image, inspect its failure message, retry after fixing it, repair a bad model cache and verify hash/error handling. Export diagnostics and inspect that private paths/text/context are absent.
- [ ] Clear index: confirm the warning; verify originals/capture sidecars/exported files remain, app metadata/feedback/index/thumbnail entries clear, watched folders become empty, scope becomes chosen folders and clipboard saving turns off. Configure a folder again to resume scanning.
- [ ] Export: select one and multiple results; cancel the folder picker; complete export and inspect the original copies plus `manifest.json`. Verify an existing file is not overwritten and unavailable-file failures are reported with an incomplete export understood.
- [ ] Capture companion: load unpacked in Chrome and Edge, capture a normal webpage with and without source context, keep the popup open until completion, verify downloads/sidecar import, reject restricted pages, change tab/page during capture and test denied/interrupted downloads. Do not use private production pages as fixtures.
- [ ] Packaging/accessibility: install/launch/uninstall the platform bundle; verify keyboard-only navigation, focus/escape behavior, list/grid layouts, resize, light/dark themes and OS accessibility controls. Record signing/security prompts as unresolved distribution issues, not bypass instructions.

## External release prerequisites

These items cannot be supplied by source changes alone. No credentials were created, uploaded or configured, no store submission was made, and no remote release was published by this implementation.

| Goal                        | Owner-provided prerequisites                                                                                                                                                                                                                                                        | Current state                                                                                            |
| --------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------- |
| Windows signed distribution | An authorized Authenticode/code-signing identity, access to its private key or hardware/cloud signing service, timestamp configuration and certificate-chain verification on clean Windows machines.                                                                                | No signing identity/workflow configured. An installer artifact alone does not establish publisher trust. |
| macOS distribution          | An enrolled Apple Developer team, Developer ID Application certificate/private key, Team ID and authorized notarization credentials (Apple ID/app-specific password or App Store Connect API key as selected by the owner), plus notarization/stapling and clean-host verification. | No signing/notarization credentials configured or success claimed.                                       |
| Automatic updates           | Owner-controlled release hosting/HTTPS update feed, updater signing private key with securely distributed public verifier, configured updater integration, and tested upgrade/rollback behavior.                                                                                    | No updater plugin/feed/signing integration configured.                                                   |
| Chrome/Edge store release   | Authorized Chrome Web Store/Edge Add-ons developer accounts, listing assets, accurate permissions/privacy disclosures, packaged extension and owner-authorized review/submission.                                                                                                   | Companion is an unpacked local directory only. Store approval and distribution remain external.          |
| Published desktop release   | Authorized repository/release write access, selected version/tag, owner-approved artifacts, release notes and download location.                                                                                                                                                    | CI builds/uploads artifacts; it does not publish a GitHub release.                                       |
| Model quality claim         | Permissioned representative corpus, reviewed relevance labels, agreed acceptance criteria, held-out model comparison, and native performance results on supported hardware.                                                                                                         | Harness/regression checks exist. They do not prove a market-leader replacement.                          |

Local use and tests do not require signing/store credentials or a model API key. Missing release credentials block those publication steps, not ordinary local development or evaluation. Keep private keys and passwords out of source, examples and test reports.

## Evidence record

Before release, append a dated record with commit/build identity, exact commands, pass/fail/blocked results, native OS checklist outcomes, permissioned corpus provenance and report locations. Distinguish synthetic browser checks, real-model retrieval, native pipeline smoke, packaged native interaction, signed distribution and remote publication. Leave unexecuted checks unchecked.
