# Magpie

**Find the work screenshot you remember, then use it immediately.** Magpie searches images in your existing folders by filename, extracted text, your notes and tags, or visual similarity. Search and indexing run on your device. The project directory is named `glint`; the current application and package name is Magpie.

The latest changes are unreleased. See [release readiness](docs/release-readiness.md) for validation commands, native QA and distribution prerequisites. Visual relevance still needs representative held-out evaluation; this project does not claim market-leader parity.

Windows 10/11 and macOS 12 or later. Free and open source.

---

## The problem

Screens are where work happens now, and screenshots are how people remember them: a receipt, a one-time code, a Slack thread, a competitor's pricing page, a design review. They pile up as `Screenshot (412).png`, and finding one later means scrolling through hundreds of thumbnails.

The tools people already have don't fix this:

| Tool                           | What it misses                                                                      |
| ------------------------------ | ----------------------------------------------------------------------------------- |
| File search (Explorer, Finder) | Searches file names, not what is inside the image.                                  |
| Cloud-backed photo workflows   | May require uploading private screens (chats, banking, work documents) to a server. |
| OS-level "recall" features     | Record everything all the time, which raised serious privacy concerns.              |
| Note apps with OCR             | Only search what you paste into them, one item at a time.                           |

**The job to be done:** "I saw it on my screen once. Let me find it again in seconds, without giving my screen to anyone."

## Who it is for

People whose work lives in screenshots: product managers collecting competitor flows, designers saving references, engineers capturing errors, students saving slides, anyone who copies OTPs, addresses and receipts from images.

## What it does

- **Choose search intent:** All combines text and visual ranks; Text searches filenames, OCR, notes and tags; Visual searches what images show. For `dog`, use Visual for pictures of a dog and Text for screenshots containing that word. Visual results depend on indexing coverage and model quality.
- **Find similar images:** drop or paste an image, choose Find similar in image details, or crop a region to search only that part. Crop selection supports dragging and keyboard percentage inputs.
- **See readiness:** settings distinguish the loaded model from the number of searchable images, show waiting reasons and unreadable files, and offer Finish indexing now. OCR and visual indexing make progress together instead of waiting for the entire OCR backlog.
- **Keep work context:** add notes, tags, manual collections and a source URL. Save named searches and reuse them from the search bar. Collections are image metadata, not folders containing copied files.
- **Filter and browse:** use visible folder/date/type controls or queries such as `ext:png`, `size:>1mb`, `path:downloads`, `width:>1920`, `date:week`, `color:red`, `has:url`, `in:"Design references"`, `tag:work`, `collection:"Summer trips"`, and `sort:largest`. More pages load from the index as you browse.
- **Control relevance:** mark an image Not relevant to hide it for that exact search, then use Restore result to undo. This stores a local query dismissal; it does not train the model.
- **Use the result:** select and copy extracted text, copy an image, open or reveal the original, open a detected link, pin, drag into another app, or export selected originals with a metadata manifest. Near-identical bursts fold into a stack.
- **Control the library:** choose watched folders or broader scanning, exclude private/unwanted folders, retry failed files, repair model downloads and export aggregate diagnostics. Clearing the local index keeps originals but clears extracted context and stops watched folders and clipboard saving until you configure them again. Saved searches and other preferences are retained.
- **Capture a webpage explicitly:** the optional [Magpie Capture companion](browser-extension/README.md) saves the visible Chrome/Edge webpage as a PNG and optional local source sidecar. It is supplied as an unpacked extension, not a published store listing.

## Start with a useful library

1. Add your screenshot folders in Settings > Library. Excluded folders and their descendants are left out even when searching everywhere.
2. Enable visual search if you want descriptions such as `a dog in a park`. The initial model download is about 156 MB; optional Sharper text on Windows downloads about 13 MB. Once cached, inference runs offline. Model repair may need another download.
3. Check visual coverage in Settings > Search. Model loaded means the model is available; only images already embedded are searchable by look. New files become searchable by name before OCR and embeddings finish.
4. Choose Finish indexing now if you want the backlog processed while using the computer or on battery. This may increase CPU use, fan noise and battery consumption. Pause remains available; normal background behavior returns after the queues finish.
5. Open a result to add context or search a crop. Source URLs can be entered manually or imported from the optional capture sidecar. Imported context does not overwrite your existing notes/source link.

For browser capture, load `browser-extension/` as an unpacked extension in Chrome or Edge, add `Downloads/Magpie Capture` to watched folders, and click Capture on a normal website. Keep its popup open until it reports success. Use image-only capture when the page URL contains private query parameters. Capture is limited to the visible viewport; it does not automatically record your screen, read browser history, capture other apps, or scroll a full page.

### Current format and privacy limits

The scanner accepts PNG, JPG/JPEG, WebP, GIF, TIF/TIFF and BMP. Animated or multipage images are treated as a still image; searching every frame/page is not supported. PDF, SVG, HEIC/HEIF, AVIF and RAW files are not indexed. Excessively large or corrupt images may remain findable by filename while extraction fails; see the failed-file list. OS OCR depends on installed language support, and the optional PaddleOCR recognizer is English.

Images, extracted text, embeddings and metadata stay local. The app has no account, telemetry or image-upload service. Optional model downloads use pinned remote model files and SHA-256 verification. Opening a source or detected link opens a website in your browser. Capture sidecars and exported originals/manifests are ordinary local files and can contain private context; removing the extension or clearing the app index does not delete those files. Diagnostic exports contain aggregate counts/model states, not paths, extracted text, notes or search queries.

## Product decisions

These are the calls that shaped the product. For each one: what I chose, what I rejected, and the evidence.

### 1. Local-first, private by design

Reading text, embedding images and searching run on the device. There are no accounts, telemetry or image uploads. Enabling optional model features requires an initial download; repairs can download again. Opening a source link is an explicit browser action.

_Why:_ the most valuable screenshots are the most private ones (chats, banking, work documents). Trust is the feature; a cloud version would be easier to build and harder to adopt.

### 2. Reading text: a two-pass design, chosen by benchmark

I tested five OCR engines on real screenshots and on rendered pages with known text before choosing.

| Engine                 | Known text: words found | Real screenshots: recall | Time per screenshot           |
| ---------------------- | ----------------------- | ------------------------ | ----------------------------- |
| PaddleOCR PP-OCRv5     | 98.4%                   | **98.7%**                | ~2 s                          |
| ocrs                   | 98.9%                   | 74.1%                    | ~1.3 s                        |
| Tesseract 5.4          | 94.7%                   | 85.8%                    | ~1.2 s                        |
| Windows built-in OCR   | 92.6%                   | 91.3%                    | **0.13 s**                    |
| Azure, Mistral (cloud) | not tested              |                          | uploads images; paid at scale |

Those historical benchmark results motivated two passes: operating-system OCR first, then optional PaddleOCR on Windows. macOS uses Apple Vision. The table describes the original evaluated corpus, not a guaranteed accuracy or latency for every language, device or screenshot. Re-run OCR evaluation on your target workload before making release claims.

### 3. Instant before complete

Magpie catalogs supported images by name and path first, then extracts text and visual meaning in the background, prioritizing recent images. OCR and visual work are interleaved. Settings expose coverage and waiting reasons so incomplete indexing is visible. The original 5,376-image catalog measurement appears below as a historical baseline.

### 4. Search relevance users can trust

Early testing showed that loose fuzzy matching felt broken: searching `savesage` returned images of the word "average". The current retrieval behavior separates intent:

- Text orders whole-word matches before partial matches and conservative OCR/spelling alternatives.
- All fuses text and visual rankings so a large text result set cannot hide every visual match.
- Folder/date/metadata filters and exact-query dismissals restrict candidates before visual scoring.
- Results retain evidence labels, and pagination applies after complete burst grouping.

See [search quality](docs/search-quality.md) for thresholds, regression coverage and the local real-model evaluation harness. Similarity scores are ranking signals, not confidence probabilities. Tests of fake vectors prove retrieval mechanics; they do not establish model accuracy.

### 5. Light enough to forget it is running

A background app that slows your laptop gets uninstalled. Memory and CPU are product requirements here, not engineering details:

- The window is created when you open it and released 30 seconds after you close it.
- Indexing runs at background priority. Large heavy-work backlogs can wait for idle time/mains power; Finish indexing now explicitly overrides those waits until completion. Rebuild and retry also use priority indexing.
- Unreadable files are recorded for inspection and retry instead of silently disappearing from the workflow.

### 6. Listening to the first user

Early feedback changed the product:

- "It disappears when I switch tabs" led to a normal window with the OS's own buttons, in the taskbar and Dock.
- "It closed when I took a screenshot" led to staying open for screenshot tools.
- "Why is this matching average?" led to the search rebuild above.

## Results

Historical measurements from the original Windows 10 release build, retained as baseline context. They have not been re-established for the current unreleased implementation; do not use them as current release acceptance evidence.

| Metric                        | Result                                 |
| ----------------------------- | -------------------------------------- |
| Search time per keystroke     | 0–23 ms over 5,000+ images             |
| Catalog every image on the PC | 5,376 images in 4 s, 27 MB peak memory |
| Read new images (OS OCR)      | ~28 images per second                  |
| New screenshot to searchable  | ~0.2 s                                 |
| Text accuracy (two-pass)      | 98.7% recall on real screenshots       |
| Memory, idle in the tray      | 18 MB                                  |
| Installer size                | 9.8 MB                                 |

## How I built it

I built Magpie as an AI-native product, working with AI coding agents (Claude Code) as the engineering team. My role was the product manager's:

- **Framing:** defining the job to be done and the non-negotiables (private, instant, light).
- **Evaluation design:** building the OCR benchmark (ground-truth pages plus real screenshots) so engine choices rested on data, not opinion.
- **Research:** studying how Everything, Elasticsearch, Meilisearch and Algolia handle instant search and typos, then turning that into product rules.
- **Trade-off calls:** accuracy against memory, completeness against speed, convenience against privacy.
- **Tight feedback loops:** using the product daily and turning each frustration into a fix the same day.

The agents wrote and tested the code (Rust, React, ONNX Runtime); I decided what to build, how to measure it, and when it was good enough to ship.

## Roadmap

- **Further tray/menu-bar workflow refinement:** the existing tray/menu already opens the app, pauses indexing and exposes settings.
- **macOS Spotlight integration:** find screenshots from ⌘Space.
- **Automatic collections:** manual collections and collection filters are implemented; automatic classification is future work.
- **Signed installers and auto-update:** neither signing nor an update feed is configured. Developer-owned credentials, release infrastructure and native QA are required before distribution claims.
- **Broader formats and model comparison:** add formats only with decoding/OCR/indexing coverage; evaluate alternate compatible model exports on held-out cases before changing the bundled model.

## Try it

Use Node 24 (the CI baseline), Rust 1.90 or newer, and platform build tools. Windows needs the MSVC C++ build tools/Windows SDK and WebView2 runtime; macOS needs Xcode command-line tools. First-time dependency/model downloads require network access.

```powershell
npm ci
npm run dev
```

For a browser-only preview with synthetic screenshots, use `npm run dev:web`. The preview does not test OS OCR, native dialogs, clipboard, file exports or the installed browser extension.

```powershell
npm run typecheck
npm run lint
npm run test:renderer
npm run test:capture
npm test
cargo check --manifest-path src-tauri/Cargo.toml --examples
npm run build:web
npm run build
```

`npm run build` writes platform bundles to `src-tauri/target/release/bundle` (`nsis/*.exe` on Windows and `dmg/*.dmg` on macOS). A successful local build is not evidence of code signing, notarization, store publication or an operational updater. The current CI checks and uploads platform artifacts; it does not publish a release or configure signing.

Use the real-model and isolated native examples in [search quality](docs/search-quality.md), then complete the [native OS QA checklist](docs/release-readiness.md#manual-native-os-qa) before release.

## Under the hood

| Layer         | Choice                                                           | Why                                                                                                 |
| ------------- | ---------------------------------------------------------------- | --------------------------------------------------------------------------------------------------- |
| App shell     | Tauri 2 (Rust)                                                   | 7x lighter at rest than the first Electron prototype (18 MB, was 138 MB); native window on each OS. |
| Search index  | SQLite FTS5, trigram                                             | Filename, OCR, note and tag retrieval alongside stored local vectors and metadata.                  |
| Text reading  | Windows.Media.Ocr / Apple Vision, then PaddleOCR on ONNX Runtime | Fast first pass, accurate second pass, all on device.                                               |
| Image meaning | CLIP ViT-B/32 (quantized)                                        | Search by what an image shows; loaded only when used.                                               |
| Interface     | React                                                            | Grid and list views, keyboard first.                                                                |

## About

Built by [A2K2005](https://github.com/A2K2005), AI Associate Product Manager. I build what I spec, and I work at the intersection of product thinking and applied AI: finding the user problem, choosing the model and the trade-offs, and shipping a product people use every day.

Ideas and inspiration: [gyotaku](https://github.com/xevrion/gyotaku), [neurasnip](https://github.com/Ayushkumar111/neurasnip) and [Everything](https://www.voidtools.com/). OCR code adapted from [system-ocr](https://github.com/Brooooooklyn/system-ocr) (MIT). PaddleOCR models are Apache-2.0; CLIP is MIT.
