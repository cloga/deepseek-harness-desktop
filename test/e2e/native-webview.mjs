import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { join } from 'node:path'
import process from 'node:process'

const endpoint = 'http://127.0.0.1:4444'
const application = process.env.DSH_E2E_APPLICATION
let sessionId

async function webdriver(method, path, body) {
  const response = await fetch(`${endpoint}${path}`, {
    method,
    headers: { 'content-type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
  })
  const payload = await response.json()
  if (!response.ok || payload.value?.error)
    throw new Error(`WebDriver ${method} ${path} failed: ${JSON.stringify(payload)}`)
  return payload.value
}

async function waitFor(callback, label, timeout = 60_000, shouldRetry = () => true) {
  const deadline = Date.now() + timeout
  let lastError
  while (Date.now() < deadline) {
    try {
      const value = await callback()
      if (value)
        return value
    }
    catch (error) {
      if (!shouldRetry(error))
        throw error
      lastError = error
    }
    await new Promise(resolve => setTimeout(resolve, 250))
  }
  throw new Error(`Timed out waiting for ${label}: ${lastError ?? 'condition not met'}`)
}

async function find(using, value) {
  return webdriver('POST', `/session/${sessionId}/element`, { using, value })
}

async function attribute(element, name) {
  return webdriver('GET', `/session/${sessionId}/element/${element['element-6066-11e4-a52e-4f735466cecf']}/attribute/${name}`)
}

async function click(element) {
  return webdriver('POST', `/session/${sessionId}/element/${element['element-6066-11e4-a52e-4f735466cecf']}/click`, {})
}

async function surfaceBounds(surface) {
  return {
    x: Number(await attribute(surface, 'data-native-x')),
    y: Number(await attribute(surface, 'data-native-y')),
    width: Number(await attribute(surface, 'data-native-width')),
    height: Number(await attribute(surface, 'data-native-height')),
  }
}

async function main() {
  try {
    assert.ok(application, 'DSH_E2E_APPLICATION is required')
    assert.ok(process.env.APPDATA, 'APPDATA is required')
    const session = await waitFor(
      async () => webdriver('POST', '/session', {
        capabilities: {
          alwaysMatch: {
            'browserName': 'wry',
            'wdio:tauriServiceOptions': { windowLabel: 'main' },
          },
        },
      }),
      'WebDriver session creation',
      90_000,
      error => error instanceof TypeError,
    )
    sessionId = session.sessionId

    const desktopLog = join(
      process.env.APPDATA,
      'io.github.hairyf.deepseek-harness-desktop',
      'logs',
      'desktop.log',
    )
    await waitFor(async () => {
      const log = await readFile(desktopLog, 'utf8')
      return log.includes('Starting Harness process:')
        && log.includes('[harness-webview] auth relay request accepted')
        && log.includes('[harness-webview] protected probe completed: status=200')
        && log.includes('E2E fixture protected request authenticated=true')
        && log.includes('E2E fixture authenticated root document')
        && log.includes('E2E fixture child script executed')
    }, 'natural local core startup evidence')

    let surface
    try {
      surface = await waitFor(async () => {
        const candidate = await find('css selector', '[data-testid="harness-native-surface"]')
        return await attribute(candidate, 'data-loaded') === 'true' ? candidate : undefined
      }, 'authenticated native surface')
    }
    catch (error) {
      const currentUrl = await webdriver('GET', `/session/${sessionId}/url`).catch(() => 'unavailable')
      const title = await webdriver('GET', `/session/${sessionId}/title`).catch(() => 'unavailable')
      const handles = await webdriver('GET', `/session/${sessionId}/window/handles`).catch(() => [])
      throw new Error(
        `${error instanceof Error ? error.message : error}; shell=${JSON.stringify({ currentUrl, title, handles })}`,
      )
    }
    const full = await surfaceBounds(surface)
    assert.ok(full.width > 500 && full.height > 300, `unexpected initial bounds: ${JSON.stringify(full)}`)

    const help = await waitFor(
      () => find('css selector', '[data-testid="desktop-help-button"]'),
      'Help button',
    )
    await click(help)
    await waitFor(async () => (await surfaceBounds(surface)).height < full.height, 'Help overlay crop')
    const helpBounds = await surfaceBounds(surface)
    assert.ok(helpBounds.width > 500 && helpBounds.height > 100, 'Help must not hide the main surface')

    const logs = await waitFor(
      () => find('css selector', '[data-testid="copy-run-logs-menu-item"]'),
      'Run Logs menu item',
    )
    await click(logs)
    await waitFor(() => find('css selector', '[data-slot="toast"]'), 'run logs toast')
    await waitFor(async () => (await surfaceBounds(surface)).height < full.height, 'toast overlay crop')
    const toastClose = await waitFor(
      () => find('css selector', '[data-slot="toast-close"]'),
      'toast close button',
    )
    await click(toastClose)
    await waitFor(async () => (await surfaceBounds(surface)).height === full.height, 'Help bounds restore')

    const settings = await waitFor(
      () => find('css selector', '[data-testid="desktop-config-button"]'),
      'Settings button',
    )
    await click(settings)
    await waitFor(() => find('css selector', '[data-slot="modal-backdrop"]'), 'Settings modal')
    await waitFor(async () => (await surfaceBounds(surface)).width === 1, 'Settings modal offscreen bounds')
    const close = await waitFor(
      () => find('css selector', '[data-slot="modal-close-trigger"]'),
      'Settings close button',
    )
    await click(close)
    await waitFor(async () => (await surfaceBounds(surface)).width === full.width, 'Settings bounds restore')
  }
  finally {
    if (sessionId !== undefined)
      await webdriver('DELETE', `/session/${sessionId}`)
  }
}

main().catch((error) => {
  console.error(error)
  process.exitCode = 1
})
