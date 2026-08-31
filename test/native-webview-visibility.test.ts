import { describe, expect, it } from 'vitest'
import {
  fitNativeWebviewAroundOverlays,
  floatingOverlayBounds,
  hasBlockingOverlay,
  shouldShowNativeWebview,
} from '../src/utils/native-webview-visibility'

describe('native WebView visibility', () => {
  it('keeps the child offscreen until auth readiness and behind full overlays', () => {
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
      querySelectorAll() {
        return []
      },
    }
    expect(hasBlockingOverlay(root)).toBe(true)
    expect(selector).toContain('modal-backdrop')
    expect(selector).not.toContain('dropdown-popover')
    expect(selector).not.toContain('toast')
  })

  it('crops around floating overlays instead of hiding the whole child', () => {
    const root = {
      querySelector() {
        return null
      },
      querySelectorAll() {
        return [
          { getBoundingClientRect: () => ({ x: 700, y: 50, width: 200, height: 250 }) },
        ]
      },
    }
    expect(floatingOverlayBounds(root)).toHaveLength(1)
    expect(fitNativeWebviewAroundOverlays(
      { x: 0, y: 40, width: 1000, height: 760 },
      floatingOverlayBounds(root),
    )).toEqual({ x: 0, y: 300, width: 1000, height: 500 })
  })

  it('projects DOMRect prototype getters into enumerable native bounds', () => {
    const dimensions = { x: 12, y: 44, width: 960, height: 720 }
    const domRectLike = Object.create({}, Object.fromEntries(
      Object.entries(dimensions).map(([key, value]) => [
        key,
        { configurable: true, enumerable: false, get: () => value },
      ]),
    ))

    const fitted = fitNativeWebviewAroundOverlays(domRectLike, [])
    expect(fitted).toEqual(dimensions)
    expect(Object.keys(fitted)).toEqual(['x', 'y', 'width', 'height'])
  })
})
