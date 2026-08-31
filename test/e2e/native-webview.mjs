import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { join } from 'node:path'
import process from 'node:process'

const endpoint = 'http://127.0.0.1:4444'
const application = process.env.DSH_E2E_APPLICATION
let sessionId

function runShellScenario(done) {
  function sleep(milliseconds) {
    return new Promise(resolve => setTimeout(resolve, milliseconds))
  }
  async function waitForValue(callback, label, timeout = 25_000) {
    const deadline = Date.now() + timeout
    let lastError
    while (Date.now() < deadline) {
      try {
        const value = callback()
        if (value)
          return value
      }
      catch (error) {
        lastError = error
      }
      await sleep(100)
    }
    throw new Error(`Timed out waiting for ${label}: ${lastError || 'condition not met'}`)
  }
  function byText(selector, text) {
    return Array.from(globalThis.document.querySelectorAll(selector))
      .find(element => element.textContent.trim() === text)
  }
  function bounds(surface) {
    return {
      x: Number(surface.dataset.nativeX),
      y: Number(surface.dataset.nativeY),
      width: Number(surface.dataset.nativeWidth),
      height: Number(surface.dataset.nativeHeight),
    }
  }

  async function run() {
    const surface = await waitForValue(() => {
      const candidate = globalThis.document.querySelector('[data-testid="harness-native-surface"]')
      return candidate?.dataset.loaded === 'true' ? candidate : undefined
    }, 'authenticated native surface')
    const full = bounds(surface)
    if (!(full.width > 500 && full.height > 300))
      throw new Error(`unexpected initial bounds: ${JSON.stringify(full)}`)

    const help = await waitForValue(() => byText('button', '帮助'), 'Help button')
    help.click()
    const helpBounds = await waitForValue(() => {
      const current = bounds(surface)
      return current.height < full.height ? current : undefined
    }, 'Help overlay crop')
    if (!(helpBounds.width > 500 && helpBounds.height > 100))
      throw new Error('Help must not hide the main surface')

    const logs = await waitForValue(
      () => byText('[role="menuitem"]', '运行日志'),
      'Run Logs menu item',
    )
    logs.click()
    await waitForValue(() => globalThis.document.querySelector('[data-slot="toast"]'), 'toast')
    await waitForValue(() => {
      const current = bounds(surface)
      return current.height < full.height ? current : undefined
    }, 'toast overlay crop')
    const toastClose = await waitForValue(
      () => globalThis.document.querySelector('[data-slot="toast-close"]'),
      'toast close button',
    )
    toastClose.click()
    await waitForValue(() => bounds(surface).height === full.height, 'Help bounds restore')

    const settings = await waitForValue(() => byText('button', '配置'), 'Settings button')
    settings.click()
    await waitForValue(
      () => globalThis.document.querySelector('[data-slot="modal-backdrop"]'),
      'Settings modal',
    )
    await waitForValue(() => bounds(surface).width === 1, 'Settings modal offscreen bounds')
    const close = await waitForValue(
      () => globalThis.document.querySelector('[data-slot="modal-close-trigger"]'),
      'Settings close button',
    )
    close.click()
    await waitForValue(() => bounds(surface).width === full.width, 'Settings bounds restore')
    return { full, helpBounds, restored: bounds(surface) }
  }
  run().then(
    value => done({ value }),
    error => done({ error: error instanceof Error ? error.stack : String(error) }),
  )
}

const shellScenario = `(${runShellScenario})(arguments[arguments.length - 1])`

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
    const scenario = await webdriver(
      'POST',
      `/session/${sessionId}/execute/async`,
      { script: shellScenario, args: [] },
    )
    assert.equal(scenario.error, undefined, scenario.error)
    assert.ok(scenario.value.full.width > 500)
    assert.equal(scenario.value.restored.width, scenario.value.full.width)
    const desktopLog = join(
      process.env.APPDATA,
      'io.github.hairyf.deepseek-harness-desktop',
      'logs',
      'desktop.log',
    )
    await waitFor(async () => {
      const log = await readFile(desktopLog, 'utf8')
      return log.includes('Starting Harness process:')
        && log.includes('E2E fixture protected request authenticated=true')
        && log.includes('E2E fixture authenticated root document')
        && log.includes('E2E fixture child script executed')
    }, 'natural local core startup evidence')
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
