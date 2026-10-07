# Changelog

## Unreleased

### Search and indexing

- Added explicit All, Text and Visual modes, fused text/visual ranking and retained evidence labels.
- Applied eligibility filters before visual scoring and folded complete bursts before backend pagination.
- Added current-model embedding versioning, stable ranking ties and a bounded photo-label prompt for short visual labels.
- Interleaved OCR/visual work with recent-image priority, exposed visual coverage/waiting reasons and added Finish indexing now.
- Added failed-file listing/retry, non-destructive rebuild behavior, model repair and pinned hash-verified downloads.
- Preserved indexed entries when folder enumeration is incomplete, and added recursive folder exclusions.

### Workflows

- Added named saved searches, visible filters and quoted tag/collection filters.
- Added local notes, tags, manual collections and source links, plus an optional visible-page browser capture companion with source-sidecar import.
- Added crop-to-search with pointer and keyboard controls, exact-query relevance dismissal/restore, original-file export with a metadata manifest, and aggregate diagnostic export.
- Added clear-index confirmation that explains retained originals and disabled watched folders/clipboard saving.
- Bound rendered results to current query intent, serialized/coalesced searches, exposed errors/retry, and disabled unavailable text-copy actions.

### Validation and release guidance

- Added renderer/capture API contract checks, retrieval/indexer/metadata/download regressions, real-model evaluation and isolated native engine smoke examples.
- Repaired CI typecheck invocation and included renderer/capture checks and example compilation in the platform workflow.
- Documented supported-format limits, model downloads/offline behavior, native OS QA, and external signing/notarization/updater/store prerequisites.

These changes are not a published release. Build artifacts, local tests and starter-corpus evaluation do not establish signed distribution, store approval, production relevance or market-leader parity. See [release readiness](docs/release-readiness.md) and [search quality](docs/search-quality.md).
