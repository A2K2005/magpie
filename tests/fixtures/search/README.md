# Local search starter corpus

Ten neutral-named images: seven public-domain/CC0 Wikimedia photos and three locally generated text screenshots. `sources.json` records each photo's source, author, license and SHA256. `synthetic-fixtures.json` records screenshot provenance and hashes. Filenames contain no object labels.

- `manifest.json`: original eight-image smoke set, 13 text/visual/all/filter queries.
- `heldout-positive-manifest.json`: adds one previously unseen dog and one cat, with nine queries. Prompt punctuation and the 0.25 threshold were frozen before those two photos were evaluated.
- `heldout-negatives.json`: original eight images with ten absent-object queries.

Use `scripts/search-eval.ps1` with cached models, then compare scheduling batch sizes 1, 4 and 8 with `scripts/search-eval-parity.ps1`. See `docs/search-quality.md` for commands and `docs/search-baseline.md` for observed results. `engine_smoke` can index this images directory through actual OS OCR in a NEW isolated profile.

Labels were manually reviewed by the implementing agent. This tiny, convenient set is a reproducible smoke test, not a representative benchmark or market-leader comparison. Manifest OCR is supplied ground truth. The native smoke separately exercises OS OCR. The backyard cat remains a known miss, and the fluffy-dog description includes a false positive. No thresholds were tuned after held-out evaluation.
