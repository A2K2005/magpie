# Search quality and local evaluation

The retrieval fixes address missing candidates and misleading ordering. They do not prove that CLIP understands every picture or that Glint matches a market leader's relevance.

## Behavior

- **All** fuses text and visual rankings using reciprocal rank, `1/(60 + position)`, and keeps both evidence labels on images matched by both. Raw BM25 and cosine are never added together. A visual match can appear near the top even when hundreds of images contain the typed word.
- **Text** searches filenames, OCR text, notes and user tags. Whole words precede partial words, then spelling/OCR corrections. It does not invoke the visual model.
- **Visual** searches current-model embeddings (legacy/other versions are excluded until reindexed) and does not add filename/OCR matches. When the model is unavailable it returns no semantic matches; readiness belongs in the UI.
- Folder, date, tags, collections, hidden images, exclusions and explicit query feedback restrict eligible images **before** vector scoring. No global top-k can exhaust a filtered folder's candidates.
- Single alphabetic labels such as `dog` use both the literal query and `a photo of a dog.`, retaining each image's highest cosine score. The complete sentence is a template in the [official CLIP prompt notebook](https://github.com/openai/CLIP/blob/main/notebooks/Prompt_Engineering_for_ImageNet.ipynb). Descriptive queries remain literal. This bounded expansion can increase false positives and still needs held-out evaluation; it is not object detection.
- Images run individually inside the reused vision session. The pinned int8 export produced different vectors when unrelated images shared its model batch, enough to move a starter dog photo across the threshold. Scheduling batches still prepare up to eight images, but each inference has shape `1 x 3 x 224 x 224`. The embedding version includes `single-image-v1`, forcing older vectors to rebuild. This trades potential batching throughput for reproducible retrieval.
- Empty/filter-only requests browse recent images in every mode without inference. Image/similarity intent requires All or Visual; Text returns an explicit error and the UI switches to Visual when initiating image search.
- The visual threshold is an absolute `0.25`; similar-image is `0.72`, blended image/text is `0.55`. These are initial operating thresholds, not calibrated probabilities. Removing the old best-minus-0.05 cutoff prevents a strong unrelated image from excluding weaker valid candidates. Low-scoring false positives still require evaluation and future calibration.
- `tag:"some tag"` and `collection:"Summer trips"` use exact case-insensitive JSON membership. Terms such as `dog tag:pets` can combine metadata and image search. Notes and tags join the existing FTS trigram index and receive the same trigger-based updates as OCR text.
- Explicit sort is applied before grouping and pagination. Text mode retains whole/partial/correction ordering. ID ties make identical scores or timestamps deterministic. Shuffle uses a deterministic ID permutation so loading another page does not reshuffle earlier images.
- Burst groups fold across the full eligible match set before slicing. `groupSize` counts matching group members; expanding a group applies the same query filters. Evidence on a folded tile is the union of matching members' evidence, so a visual label may describe a sibling image in that burst.
- Pages use `offset`, a page size of 1 to 500, `hasMore` and `nextOffset`. They are stable for an unchanged index and request. Indexing, edits, pins, hidden images or new files can move positions, so clients should refresh the first page when index state changes. This is not snapshot/cursor isolation.

Correctness currently materializes matching rows and eligible vectors on each request. This is O(matches), and avoids the former 200/400/80/300 result ceilings. OCR geometry is parsed only for the returned page. Profile large libraries before introducing cursor storage or a filtered approximate-nearest-neighbor index.

## Regression checks

Run `cargo test --manifest-path src-tauri/Cargo.toml --lib engine::search` and `cargo test --manifest-path src-tauri/Cargo.toml --lib engine::query`.

The fake-vector checks also prove Text/browse requests never infer, reject Text+image intent, exclude incompatible model versions, and preserve raw recall when the single-label photo prompt adds candidates.

The fake-vector checks are deterministic tests of retrieval mechanics, not model accuracy. They cover:

- A filtered valid image ranked behind 359 globally stronger images.
- A valid absolute score retained despite a much higher best score.
- Mode segregation, dual evidence, and visual rank amid 349 text hits.
- Complete grouping of an 801-image burst and pagination beyond the old limits.
- Metadata search and filters, explicit query dismissal, no matches, stable score ties and exhaustion.

## Evaluate the actual model locally

The native example uses Glint's real CLIP preprocessing, model inference, quantized vectors and search implementation. It creates a separate in-memory database, does not open your library database and does not upload images. Model files must already be downloaded through the app. It performs no model downloads itself.

```powershell
.\scripts\search-eval.ps1 -Models 'C:\path\to\models' -Manifest 'C:\path\to\corpus\manifest.json' -Report 'C:\path\to\report.json'
```

Or run `cargo run --manifest-path src-tauri/Cargo.toml --example search_eval -- <models_dir> <manifest.json> <report.json> [batch_size]`. Scheduling batch size defaults to 8, with allowed values 1 to 8. The PowerShell wrapper accepts `-BatchSize`.

Manifest paths are relative to the manifest. Use images you may lawfully evaluate and record their provenance in `description`. Choose neutral filenames if measuring visual ability versus filename search.

```json
{
  "description": "Locally curated, permissioned photos and screenshots. Labels reviewed manually.",
  "images": [
    {"id": "photo-dog", "path": "images/001.jpg", "tags": ["pets"]},
    {"id": "dog-text", "path": "images/002.png", "text": "A document about dog food"},
    {"id": "cat", "path": "images/003.jpg"}
  ],
  "queries": [
    {"query": "dog", "mode": "visual", "relevant": ["photo-dog"], "k": 3},
    {"query": "dog", "mode": "text", "relevant": ["dog-text"], "k": 3},
    {"query": "dog", "mode": "all", "relevant": ["photo-dog", "dog-text"], "k": 3},
    {"query": "dog tag:pets", "mode": "visual", "relevant": ["photo-dog"], "k": 3},
    {"query": "a helicopter", "mode": "visual", "relevant": [], "k": 3}
  ]
}
```

For each query, the report records returned IDs, scores, evidence, latency, precision@k, recall@k and reciprocal rank. Queries with no relevant images report `no_match_correct`; recall/MRR are null. Precision uses k as denominator even if fewer images return. Ground-truth OCR text is supplied by the manifest, so this does **not** evaluate OCR extraction accuracy. Model files include SHA256 hashes, byte sizes and modification times for run provenance. Per-image embedding fingerprints allow exact batch parity checks:

```powershell
.\scripts\search-eval-parity.ps1 -Reports batch1.json,batch4.json,batch8.json
```

This fails on different model files, embedding contracts, image vectors, query ordering, scores or evidence. Run the same manifest and models with each scheduling batch size first.

## Release evidence still required

Use a permissioned, representative corpus with distractors and reviewed labels. Include dogs/cats and fine distinctions, screens containing the word without the object, UI components, charts, receipts, code, low-text images, folders with limited candidates, quoted filters, ambiguous queries, and true no-match queries. Separate warm and cold latency; record hardware and index coverage. Evaluate text, visual and all modes against the same labels, and compare alternative model exports by changing the model directory only when their tokenizer/input/output contracts match.

A small public/synthetic starter corpus establishes that the model runs and helps expose obvious failures. Its scores are not a production relevance rate. Do not tune thresholds on the same cases used to report release quality; keep held-out cases. Define acceptance targets from observed baseline and user feedback before claiming market-leader parity.

## Native indexing smoke

`cargo run --manifest-path src-tauri/Cargo.toml --example engine_smoke -- <cached_models_dir> <starter_fixture_images_dir> <NEW_profile_dir> <report.json>` creates a fresh profile, copies the cached models into it, and runs the actual Engine scanner, OS OCR, image analysis, thumbnail writing, embedding and search. It asserts the starter corpus's real dog photos (001/002) and invoice screenshot (007) are retrieved. The supplied folder must be the starter fixture image directory. It keeps the profile for inspection and refuses to reuse existing directories. This proves the native pipeline on fixtures separately from mock browser tests and the manifest-ground-truth relevance harness.
