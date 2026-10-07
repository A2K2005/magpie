# Magpie

**It remembers every image you've seen.** Like the bird that collects shiny things, Magpie keeps track of every screenshot and image on your computer. Find any image on your computer by the words in it, by what it shows, or by its name. Results appear as you type, in milliseconds, and nothing ever leaves your machine.

Windows 10/11 and macOS 12 or later. Free and open source.

---

## The problem

Screens are where work happens now, and screenshots are how people remember them: a receipt, a one-time code, a Slack thread, a competitor's pricing page, a design review. They pile up as `Screenshot (412).png`, and finding one later means scrolling through hundreds of thumbnails.

The tools people already have don't fix this:

| Tool | What it misses |
|---|---|
| File search (Explorer, Finder) | Searches file names, not what is inside the image. |
| Cloud photo apps | Upload private screens (chats, banking, work documents) to a server. |
| OS-level "recall" features | Record everything all the time, which raised serious privacy concerns. |
| Note apps with OCR | Only search what you paste into them, one item at a time. |

**The job to be done:** "I saw it on my screen once. Let me find it again in seconds, without giving my screen to anyone."

## Who it is for

People whose work lives in screenshots: product managers collecting competitor flows, designers saving references, engineers capturing errors, students saving slides, anyone who copies OTPs, addresses and receipts from images.

## What it does

- **Search the text inside images.** Type a word that appears anywhere in a screenshot and Magpie lights it up on the thumbnail.
- **Forgives typos, without guessing wildly.** Type `saceage` and it says "Showing results for **savesage**", the way Google does. A word that exists in your images is never "corrected" into something else.
- **Search by what an image shows.** Type `dog`, `receipt` or `code editor` and it finds images that look like that, even with no text in them. Drop in an image to find similar ones.
- **Every image on your computer, or just your screenshots.** One switch catalogs every image on your drives (system and app folders skipped). New images are findable by name at once; text and visual search fill in over time.
- **Filters like a power tool:** `ext:png`, `size:>1mb`, `path:downloads`, `width:>1920`, `date:week`, `color:red`, `has:url`, `in:discord`, `sort:largest`.
- **Act on what it finds:** copy the text, copy a one-time code, open a link, copy the image, or drag it straight into Slack or Figma.
- **Bursts fold into one tile,** so ten near-identical screenshots taken a minute apart show as one stack.

## Product decisions

These are the calls that shaped the product. For each one: what I chose, what I rejected, and the evidence.

### 1. Local-first, private by design

Everything (reading text, understanding images, search) runs on the device. There are no accounts, no telemetry and no uploads. The only network use is a one-time model download, and only for features that need it.

*Why:* the most valuable screenshots are the most private ones (chats, banking, work documents). Trust is the feature; a cloud version would be easier to build and harder to adopt.

### 2. Reading text: a two-pass design, chosen by benchmark

I tested five OCR engines on real screenshots and on rendered pages with known text before choosing.

| Engine | Known text: words found | Real screenshots: recall | Time per screenshot |
|---|---|---|---|
| PaddleOCR PP-OCRv5 | 98.4% | **98.7%** | ~2 s |
| ocrs | 98.9% | 74.1% | ~1.3 s |
| Tesseract 5.4 | 94.7% | 85.8% | ~1.2 s |
| Windows built-in OCR | 92.6% | 91.3% | **0.13 s** |
| Azure, Mistral (cloud) | not tested | | uploads images; paid at scale |

No single engine was both fast and accurate, so Magpie uses two passes: the operating system's OCR makes a new screenshot searchable in about 0.2 s, then PaddleOCR re-reads it in the background for the last 7% of accuracy. On macOS, Apple Vision is already accurate enough, so the second pass is skipped.

### 3. Instant before complete

Users judge a search tool in the first ten seconds. Magpie lists every image by name and path first (5,376 images across a full PC in 4 seconds), then reads text and meaning in the background, newest first. Search works from the first second and gets smarter over time.

### 4. Search relevance users can trust

Early testing showed that loose fuzzy matching felt broken: searching `savesage` returned images of the word "average". I rebuilt the search on the same principles as Elasticsearch's "did you mean", Meilisearch and Algolia:

- **Matches first, then Suggested.** `cred` shows "Switch CRED…" before "Credila".
- **Only unknown words are corrected,** to the closest real word in your images, with the first letter right.
- **Say so when results are for a correction,** so the user always knows why an image appeared.

### 5. Light enough to forget it is running

A background app that slows your laptop gets uninstalled. Memory and CPU are product requirements here, not engineering details:

- The window is created when you open it and released 30 seconds after you close it.
- Indexing runs at background priority, and heavy work waits until you step away and are on mains power.
- One unreadable file can never crash the app.

### 6. Listening to the first user

Early feedback changed the product:

- "It disappears when I switch tabs" led to a normal window with the OS's own buttons, in the taskbar and Dock.
- "It closed when I took a screenshot" led to staying open for screenshot tools.
- "Why is this matching average?" led to the search rebuild above.

## Results

Measured on a Windows 10 laptop (release build).

| Metric | Result |
|---|---|
| Search time per keystroke | 0–23 ms over 5,000+ images |
| Catalog every image on the PC | 5,376 images in 4 s, 27 MB peak memory |
| Read new images (OS OCR) | ~28 images per second |
| New screenshot to searchable | ~0.2 s |
| Text accuracy (two-pass) | 98.7% recall on real screenshots |
| Memory, idle in the tray | 18 MB |
| Installer size | 9.8 MB |

## How I built it

I built Magpie as an AI-native product, working with AI coding agents (Claude Code) as the engineering team. My role was the product manager's:

- **Framing:** defining the job to be done and the non-negotiables (private, instant, light).
- **Evaluation design:** building the OCR benchmark (ground-truth pages plus real screenshots) so engine choices rested on data, not opinion.
- **Research:** studying how Everything, Elasticsearch, Meilisearch and Algolia handle instant search and typos, then turning that into product rules.
- **Trade-off calls:** accuracy against memory, completeness against speed, convenience against privacy.
- **Tight feedback loops:** using the product daily and turning each frustration into a fix the same day.

The agents wrote and tested the code (Rust, React, ONNX Runtime); I decided what to build, how to measure it, and when it was good enough to ship.

## Roadmap

- **Menu bar and tray quick search:** click an icon at the top of the screen, type, and drag the image out without opening a window.
- **macOS Spotlight integration:** find screenshots from ⌘Space.
- **Smarter organisation:** automatic collections such as "Receipts" and "One-time codes".
- **Signed installers** and auto-update.

## Try it

Requirements: Node 20 or later, Rust (stable) and, on Windows, the MSVC Build Tools.

```bash
npm install
npm run build
```

The installer is written to `src-tauri/target/release/bundle` (`.exe` on Windows, `.dmg` on macOS). For development, run `npm run dev`. Unit tests: `npm test`.

## Under the hood

| Layer | Choice | Why |
|---|---|---|
| App shell | Tauri 2 (Rust) | 7x lighter at rest than the first Electron prototype (18 MB, was 138 MB); native window on each OS. |
| Search index | SQLite FTS5, trigram | Substring search over names and text in milliseconds, in one file. |
| Text reading | Windows.Media.Ocr / Apple Vision, then PaddleOCR on ONNX Runtime | Fast first pass, accurate second pass, all on device. |
| Image meaning | CLIP ViT-B/32 (quantized) | Search by what an image shows; loaded only when used. |
| Interface | React | Grid and list views, keyboard first. |

## About

Built by [A2K2005](https://github.com/A2K2005), AI Associate Product Manager. I build what I spec, and I work at the intersection of product thinking and applied AI: finding the user problem, choosing the model and the trade-offs, and shipping a product people use every day.

Ideas and inspiration: [gyotaku](https://github.com/xevrion/gyotaku), [neurasnip](https://github.com/Ayushkumar111/neurasnip) and [Everything](https://www.voidtools.com/). OCR code adapted from [system-ocr](https://github.com/Brooooooklyn/system-ocr) (MIT). PaddleOCR models are Apache-2.0; CLIP is MIT.
