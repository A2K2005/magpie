/* global chrome */
const button = document.querySelector('#capture')
const status = document.querySelector('#status')

async function download(url, filename) {
  const id = await chrome.downloads.download({ url, filename, saveAs: false, conflictAction: 'uniquify' })
  // Register first, then check state: downloads may complete before the listener is attached.
  await new Promise((resolve, reject) => {
    const timeout = setTimeout(() => done(new Error('Download is taking too long. Check browser Downloads.')), 30000)
    const changed = (change) => {
      if (change.id !== id) return
      if (change.state?.current === 'complete') done()
      if (change.state?.current === 'interrupted') done(new Error('Download was interrupted. Check browser Downloads.'))
    }
    function done(error) {
      clearTimeout(timeout)
      chrome.downloads.onChanged.removeListener(changed)
      if (error) reject(error)
      else resolve()
    }
    chrome.downloads.onChanged.addListener(changed)
    chrome.downloads.search({ id }).then(([item]) => {
      if (item?.state === 'complete') done()
      if (item?.state === 'interrupted') done(new Error('Download was interrupted.'))
    }, done)
  })
}

button.addEventListener('click', async () => {
  button.disabled = true
  status.textContent = 'Capturing. Keep this popup open until the files are saved.'
  try {
    const [tab] = await chrome.tabs.query({ active: true, currentWindow: true })
    if (!tab) throw new Error('No active page is available.')
    const source = new URL(tab.url)
    if (!['http:', 'https:'].includes(source.protocol)) throw new Error('Open a regular website to capture it.')
    const image = await chrome.tabs.captureVisibleTab(tab.windowId, { format: 'png' })
    const [current] = await chrome.tabs.query({ active: true, currentWindow: true })
    if (current?.id !== tab.id || current?.url !== tab.url) throw new Error('The page changed during capture. Try again.')
    const stem = `Capture ${new Date().toISOString().replace(/[:.]/g, '-')}-${crypto.randomUUID().slice(0, 8)}`
    const name = `${stem}.png`
    if (document.querySelector('#context').checked) {
      source.username = ''
      source.password = ''
      source.hash = ''
      const data = JSON.stringify({ version: 1, imageName: name, sourceUrl: source.href, title: tab.title || '', capturedAt: new Date().toISOString() }, null, 2)
      await download(`data:application/json;charset=utf-8,${encodeURIComponent(data)}`, `Magpie Capture/${stem}.magpie.json`)
    }
    await download(image, `Magpie Capture/${name}`)
    status.textContent = 'Saved to Downloads / Magpie Capture. Magpie will index it when that folder is watched.'
  } catch (error) {
    status.textContent = error.message || 'Capture failed. Please try again.'
  } finally {
    button.disabled = false
  }
})
