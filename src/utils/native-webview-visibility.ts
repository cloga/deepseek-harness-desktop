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
  let fitted = { ...base }
  for (const overlay of overlays) {
    const right = fitted.x + fitted.width
    const bottom = fitted.y + fitted.height
    const overlayRight = overlay.x + overlay.width
    const overlayBottom = overlay.y + overlay.height
    const intersects = overlay.x < right
      && overlayRight > fitted.x
      && overlay.y < bottom
      && overlayBottom > fitted.y
    if (!intersects)
      continue
    const above = Math.max(0, overlay.y - fitted.y)
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
  return fitted
}

export function shouldShowNativeWebview(
  nativeWebview: boolean,
  serviceHealthy: boolean,
  iframeLoaded: boolean,
  blockingOverlay: boolean,
): boolean {
  return nativeWebview && serviceHealthy && iframeLoaded && !blockingOverlay
}
