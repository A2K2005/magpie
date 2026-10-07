// Browser API contract tests. These do not claim a store-installed extension test.
import { readFile } from 'node:fs/promises'
import { runInNewContext } from 'node:vm'
import { test } from 'node:test'
import assert from 'node:assert/strict'

const source = await readFile(new URL('../browser-extension/popup.js', import.meta.url), 'utf8')

function harness({ context = true, changed = false, failDownload = false, url = 'https://example.com/private?q=dog#section' } = {}) {
  let click
  let queries = 0
  const downloads = []
  const listeners = new Set()
  const button = { disabled: false, addEventListener: (_, fn) => { click = fn } }
  const status = { textContent: '' }
  const tab = { id: 1, windowId: 2, url, title: 'Dog reference' }
  const chrome = {
    tabs: {
      query: async () => [{ ...tab, id: changed && ++queries > 1 ? 2 : 1 }],
      captureVisibleTab: async () => 'data:image/png;base64,fixture'
    },
    downloads: {
      download: async (options) => {
        if (failDownload) throw new Error('Download denied')
        downloads.push(options)
        return downloads.length
      },
      onChanged: { addListener: (fn) => listeners.add(fn), removeListener: (fn) => listeners.delete(fn) },
      search: async () => [{ state: 'complete' }]
    }
  }
  runInNewContext(source, {
    chrome, document: { querySelector: (selector) => ({ '#capture': button, '#status': status, '#context': { checked: context } })[selector] },
    URL, Date, crypto: { randomUUID: () => '12345678-1234-1234-1234-123456789abc' }, setTimeout, clearTimeout
  })
  return { run: () => click(), button, status, downloads, listeners }
}

test('capture writes matching context before image and strips credentials/hash', async () => {
  const h = harness({ url: 'https://user:password@example.com/private?q=dog#section' })
  await h.run()
  assert.equal(h.downloads.length, 2)
  const sidecar = JSON.parse(decodeURIComponent(h.downloads[0].url.split(',').slice(1).join(',')))
  assert.equal(sidecar.sourceUrl, 'https://example.com/private?q=dog')
  assert.equal(sidecar.imageName, h.downloads[1].filename.split('/').at(-1))
  assert.match(h.downloads[0].filename, /\.magpie\.json$/)
  assert.match(h.status.textContent, /^Saved/)
  assert.equal(h.button.disabled, false)
  assert.equal(h.listeners.size, 0)
})

test('context opt-out saves only the screenshot', async () => {
  const h = harness({ context: false })
  await h.run()
  assert.equal(h.downloads.length, 1)
  assert.match(h.downloads[0].url, /^data:image/)
})

test('navigation during capture aborts without misattributed files', async () => {
  const h = harness({ changed: true })
  await h.run()
  assert.equal(h.downloads.length, 0)
  assert.match(h.status.textContent, /page changed/)
})

test('download failure never claims success and permits retry', async () => {
  const h = harness({ failDownload: true })
  await h.run()
  assert.equal(h.downloads.length, 0)
  assert.equal(h.status.textContent, 'Download denied')
  assert.equal(h.button.disabled, false)
})

test('browser internal pages are rejected before download', async () => {
  const h = harness({ url: 'chrome://settings' })
  await h.run()
  assert.equal(h.downloads.length, 0)
  assert.match(h.status.textContent, /regular website/)
})
