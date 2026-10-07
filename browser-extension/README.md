# Magpie Capture

A local-only Chrome/Edge Manifest V3 companion. It captures the visible page when explicitly clicked, saves a PNG and optional source context beside it, and uses no network service or automatic background capture.

Load this directory as an unpacked extension through your browser's extension developer screen. Add `Downloads/Magpie Capture` to Magpie's chosen folders, then use the toolbar popup. Keep the popup open until it reports success. Installation and extension-store publication are not performed by the build.

The `.magpie.json` sidecar is written first and contains the image filename, page URL, title, and capture time. Magpie validates the sidecar and imports the URL and title without overwriting user-authored metadata. Page URLs may contain sensitive query parameters; the popup offers image-only capture. Captures are normal local files. Removing the extension or clearing Magpie's index does not delete them.

Scope: visible viewport only. It does not capture full-page scroll, browser chrome, video, or other applications. It does not read browsing history. Browser download prompts/policies can prevent unattended saves; failures are reported rather than treated as success. Content deduplication and signed store distribution are future work, not claimed here.

API references: https://developer.chrome.com/docs/extensions/develop/concepts/activeTab and https://developer.chrome.com/docs/extensions/reference/api/downloads.
