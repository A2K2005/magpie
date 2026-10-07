# Native search evidence, 2026-10-08

These local Windows runs use the pinned Xenova CLIP ViT-B/32 int8 files and Glint's actual preprocessing, normalized vectors, database serialization and search. They never open the installed library database or upload images. Corpus and provenance are in `tests/fixtures/search`. This is a smoke set, not a production accuracy estimate.

## What was observed

The initial raw-query baseline on eight images missed both real dog photos for `dog` and returned one of two cats for `cat`. A bounded raw-plus-photo sentence prompt improved these cases. The final prompt is the literal query plus `a photo of a {label}.` for a single alphabetic label only, taking each image's maximum cosine. Descriptive queries remain literal. The absolute 0.25 threshold was unchanged.

The quantized vision export also produced different image representations depending on other images in the inference batch. With the final prompt but the old batch inference, batch 4 missed one of two original dog photos; batch 1 and batch 8 found both. Adding the held-out photos changed the relative order of the dogs under batch 8. This is a reproducibility defect regardless of relevance labels.

The final implementation runs one image per inference inside a reused session, retaining caller scheduling batches. Its version is `clip-vit-b32-int8-d15189d7-single-image-v1`; older vectors are excluded and queued for reindexing.

| Final check | Observed result |
| --- | --- |
| Original 8 images, 13 queries, scheduling batches 1/4/8 | Exact per-image vector SHA256, returned IDs, scores and evidence identical across all three reports |
| Held-out 10 images, 9 queries, scheduling batches 1/4/8 | Same exact parity |
| `dog`, Visual, held-out set | All 3 dog photos retrieved, including previously unseen 009.jpg; no text-only dog document returned |
| `cat`, Visual, held-out set | 2 of 3 cats retrieved; small cat in backyard photo 010.jpg missed in every batch |
| `dog`, Text | Only the dog-food document returned |
| `dog`, All | The 3 dog photos and dog-food document returned |
| Original absent-object set | All 10 queries returned no results |
| Held-out absent-object cases | All 5 queries returned no results in every scheduling batch |
| `a white fluffy dog`, original set | Correct Samoyed ranked first, but Corgi also returned as a false positive |

The unseen dog/cat images were added after the prompt and threshold were frozen. Neither the missed cat nor the Corgi false positive was used to adjust them. Small subjects, crops, UI screenshots and hard negatives need a larger independent evaluation before release claims.

## Throughput tradeoff

`indexing_ms` in the relevance harness includes image decoding, preprocessing and cold vision-session startup. These are single local measurements with background builds active, not a controlled performance benchmark.

| Images / scheduling batch | Previous batched inference | Final single-image inference |
| --- | ---: | ---: |
| 8 / 1 | 2265 ms | 2369 ms |
| 8 / 4 | 1306 ms | 2490 ms |
| 8 / 8 | 1252 ms | 2218 ms |
| 10 / 1 | 2756 ms | 2840 ms |
| 10 / 4 | 2799 ms | 3059 ms |
| 10 / 8 | 2855 ms | 2900 ms |

The eight-image batch-8 measurement increased by 966 ms, about 77%; the ten-image measurement was nearly unchanged because other costs dominated. Do not infer steady-state indexing throughput from either. Determinism is the explicit tradeoff; future speed improvements must pass vector/retrieval parity or use a new evaluated embedding contract.

## Actual Engine smoke

A NEW isolated profile indexed all ten fixture files through the real folder scan, OS OCR, analysis, thumbnail generation and embedding queues. It reached idle with 10/10 OCR complete, 10/10 current-version embeddings, zero errors and no warnings. Visual `dog` returned 002.jpg, 009.jpg and 001.jpg; Text `invoice` retrieved 007.png. Thumbnail existence was checked for every returned recent tile. Indexing took 12055 ms; total smoke including queries and diagnostics took 13277 ms. Existing app data was untouched.

The final native library suite passed **38/38 tests**. Fake-vector regressions cover filtered recall behind 359 stronger out-of-filter candidates, absence of relative cutoffs, text/visual segregation, dual evidence and rank fusion, 801-member grouping, pagination beyond old caps, metadata and quoted filters, dismissals, no-match cases, and embedding-version exclusion. These tests establish mechanics, not model relevance.

## Preserved run artifacts

The orchestrator's evaluation output bundle retains:

- `report.json`: original literal-query baseline.
- `expanded-report.json`, `article-report.json`: exploratory prompt runs, retained as development evidence rather than held-out validation.
- `batch1-report.json`, `batch4-report.json`, `batch8-report.json` and `heldout-positive-batch*.json`: pre-fix batch-dependent results.
- `single-image-batch1.json`, `single-image-batch4.json`, `single-image-batch8.json`: final original-set results.
- `single-image-heldout-batch1.json`, `single-image-heldout-batch4.json`, `single-image-heldout-batch8.json`: final held-out results.
- `single-image-negatives.json`: final ten absent-object checks.
- `native-single-image-report.json`: fresh-profile native pipeline status, OS OCR and searches.

Reports contain actual model-file SHA256 values and final reports include per-image vector fingerprints. Reproduce using the committed fixtures and the commands in `search-quality.md`. Market-leader parity remains unverified; a larger permissioned corpus, reviewed labels, user tasks and controlled latency measurements are still required.
