import { readFileSync } from 'node:fs'
import { describe, expect, it } from 'vitest'

describe('native WebView E2E shell bootstrap', () => {
  it('waits synchronously for authenticated readiness before starting async work', () => {
    const source = readFileSync(
      new URL('./e2e/native-webview.mjs', import.meta.url),
      'utf8',
    )
    const bootstrap = source
      .split('function installShellScenario')[1]
      ?.split('function shellBootstrap')[0]

    expect(bootstrap).toContain('MutationObserver(startWhenAuthenticated)')
    expect(bootstrap).toContain('surface?.dataset.loaded !== \'true\'')
    expect(bootstrap).toContain('scenario(callbackUrl, surface)')
    expect(bootstrap).not.toContain('setTimeout')
    expect(bootstrap).not.toContain('Promise')
  })
})
