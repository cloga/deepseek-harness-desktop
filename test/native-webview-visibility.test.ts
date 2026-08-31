import { describe, expect, it } from 'vitest'
import { hasBlockingOverlay, shouldShowNativeWebview } from '../src/utils/native-webview-visibility'

describe('native WebView visibility', () => {
  it('keeps the child hidden until auth readiness and hides it behind overlays', () => {
    expect(shouldShowNativeWebview(true, true, false, false)).toBe(false)
    expect(shouldShowNativeWebview(true, true, true, false)).toBe(true)
    expect(shouldShowNativeWebview(true, true, true, true)).toBe(false)
  })

  it('recognizes a blocking overlay through the shared selector', () => {
    let selector = ''
    const root = {
      querySelector(value: string) {
        selector = value
        return { overlay: true }
      },
    }
    expect(hasBlockingOverlay(root)).toBe(true)
    expect(selector).toContain('modal-backdrop')
    expect(selector).toContain('dropdown-popover')
    expect(selector).toContain('toast')
  })
})
