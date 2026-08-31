const BLOCKING_OVERLAY_SELECTOR = [
  '[data-slot="modal-backdrop"]',
  '[data-slot="alert-dialog-backdrop"]',
  '[data-slot="drawer-backdrop"]',
].join(',')

const FLOATING_OVERLAY_SELECTOR = [
  '[data-slot="dropdown-popover"]',
  '[data-slot$="-popover"]',
  '[data-slot="popover-dialog"]',
  '[data-slot="toast"]',
].join(',')

interface QueryRoot {
  querySelector: (selectors: string) => unknown
  querySelectorAll: (selectors: string) => Iterable<{ getBoundingClientRect: () => NativeWebviewBounds }>
}

export interface NativeWebviewBounds {
  x: number
  y: number
  width: number
  height: number
}

export function serializeNativeWebviewBounds(
  bounds: NativeWebviewBounds,
): NativeWebviewBounds & Record<string, unknown> {
  const serialized: NativeWebviewBounds & Record<string, unknown> = {
    x: bounds.x,
    y: bounds.y,
    width: bounds.width,
    height: bounds.height,
  }
  if (!Object.values(serialized).every(Number.isFinite))
    throw new Error(`NATIVE_WEBVIEW_BOUNDS_INVALID: ${JSON.stringify(serialized)}`)
  return serialized
}

export function hasBlockingOverlay(root: QueryRoot): boolean {
  return root.querySelector(BLOCKING_OVERLAY_SELECTOR) !== null
}

export function floatingOverlayBounds(root: QueryRoot): NativeWebviewBounds[] {
  return Array.from(root.querySelectorAll(FLOATING_OVERLAY_SELECTOR))
    .map(element => element.getBoundingClientRect())
}

export function fitNativeWebviewAroundOverlays(
  base: NativeWebviewBounds,
  overlays: NativeWebviewBounds[],
): NativeWebviewBounds {
  let fitted = serializeNativeWebviewBounds(base)
  for (const overlay of overlays) {
    const safeOverlay = serializeNativeWebviewBounds(overlay)
    const right = fitted.x + fitted.width
    const bottom = fitted.y + fitted.height
    const overlayRight = safeOverlay.x + safeOverlay.width
    const overlayBottom = safeOverlay.y + safeOverlay.height
    const intersects = safeOverlay.x < right
      && overlayRight > fitted.x
      && safeOverlay.y < bottom
      && overlayBottom > fitted.y
    if (!intersects)
      continue
    const above = Math.max(0, safeOverlay.y - fitted.y)
    const below = Math.max(0, bottom - overlayBottom)
    if (below >= above) {
      fitted = {
        ...fitted,
        y: Math.min(bottom, overlayBottom),
        height: Math.max(1, below),
      }
    }
    else {
      fitted = {
        ...fitted,
        height: Math.max(1, above),
      }
    }
  }
  return serializeNativeWebviewBounds(fitted)
}

export function shouldShowNativeWebview(
  nativeWebview: boolean,
  serviceHealthy: boolean,
  iframeLoaded: boolean,
  blockingOverlay: boolean,
): boolean {
  return nativeWebview && serviceHealthy && iframeLoaded && !blockingOverlay
}

export function shouldSyncNativeWebviewBounds(
  nativeWebview: boolean,
  nativeWebviewMounted: boolean,
  iframeLoaded: boolean,
): boolean {
  return nativeWebview && nativeWebviewMounted && iframeLoaded
}
