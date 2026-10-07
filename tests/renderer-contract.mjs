// Run with node tests/renderer-contract.mjs. Browser preview contract, not native search evaluation.
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import ts from 'typescript'

globalThis.location = { search: '?copies=8&model=ready' }
globalThis.window = { setInterval: () => 0, setTimeout }
const source = await readFile(new URL('../src/renderer/src/lib/mockApi.ts', import.meta.url), 'utf8')
const { outputText } = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ES2022 } })
const { createMockApi } = await import(`data:text/javascript;base64,${Buffer.from(outputText).toString('base64')}`)
const api = createMockApi()
const settings = await api.getSettings()
assert.deepEqual(settings.savedSearches, [])
assert.deepEqual(settings.excludedFolders, [])
const first = await api.search({ q: '', limit: 7 })
assert.equal(first.hits.length, 7)
assert.equal(first.hasMore, true)
assert.equal(first.nextOffset, 7)
const second = await api.search({ q: '', limit: 7, offset: first.nextOffset })
assert.equal(second.hits.length, 7)
assert.equal(first.hits.some((a) => second.hits.some((b) => a.shot.id === b.shot.id)), false)
const text = await api.search({ q: 'receipt', mode: 'text' })
assert.ok(text.hits.length > 0)
assert.equal(text.hits.some((h) => h.match === 'visual'), false)
const visual = await api.search({ q: 'sunset', mode: 'visual' })
assert.ok(visual.hits.length > 0)
assert.ok(visual.hits.every((h) => h.match === 'visual'))
const id = first.hits[0].shot.id
const metadata = { note: 'Context', tags: ['work'], collections: ['Design ideas'], sourceUrl: 'https://example.com/source' }
assert.deepEqual(await api.updateMetadata(id, metadata), metadata)
assert.deepEqual((await api.getShot(id)).metadata, metadata)
assert.deepEqual((await api.search({ q: 'tag:work' })).hits.map((h) => h.shot.id), [id])
assert.deepEqual((await api.search({ q: 'collection:"Design ideas"' })).hits.map((h) => h.shot.id), [id])
await api.setRelevant(visual.hits[0].shot.id, 'sunset', false)
assert.equal((await api.search({ q: 'sunset', mode: 'visual' })).hits.some((h) => h.shot.id === visual.hits[0].shot.id), false)
await api.setRelevant(visual.hits[0].shot.id, 'sunset', true)
assert.equal((await api.search({ q: 'sunset', mode: 'visual' })).hits.some((h) => h.shot.id === visual.hits[0].shot.id), true)
await api.finishIndexing(true)
assert.equal((await api.status()).forceIndexing, true)
await api.pause(true)
await api.clearIndex()
const cleared = await api.getSettings()
assert.deepEqual(cleared.folders, [])
assert.equal(cleared.scope, 'folders')
assert.equal(cleared.saveClipboard, false)
assert.equal((await api.search({q:''})).hits.length, 0)
assert.equal((await api.status()).errors, 0)
const utilitySource = await readFile(new URL('../src/renderer/src/lib/util.ts', import.meta.url), 'utf8')
const utilities = ts.transpileModule(utilitySource, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ES2022 } })
const { savedSearchName } = await import(`data:text/javascript;base64,${Buffer.from(utilities.outputText).toString('base64')}`)
for (const query of ['normal search', 'x'.repeat(400), '🐕'.repeat(100), 'विज़ुअल '.repeat(100)]) {
  const name = savedSearchName(query)
  assert.ok(new TextEncoder().encode(name).length <= 128)
  assert.ok(!name.includes('�'))
}
console.log('Renderer preview contract checks passed: pagination, modes, metadata, feedback and controls.')
