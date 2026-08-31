const BLOCKING_OVERLAY_SELECTOR = [
  '[data-slot="modal-backdrop"]',
  '[data-slot="alert-dialog-backdrop"]',
  '[data-slot="drawer-backdrop"]',
  '[data-slot="dropdown-popover"]',
  '[data-slot$="-popover"]',
  '[data-slot="popover-dialog"]',
  '[data-slot="toast"]',
].join(',')

interface QueryRoot {
  querySelector: (selectors: string) => unknown
}

export function hasBlockingOverlay(root: QueryRoot): boolean {
  return root.querySelector(BLOCKING_OVERLAY_SELECTOR) !== null
}

export function shouldShowNativeWebview(
  nativeWebview: boolean,
  serviceHealthy: boolean,
  iframeLoaded: boolean,
  blockingOverlay: boolean,
): boolean {
  return nativeWebview && serviceHealthy && iframeLoaded && !blockingOverlay
}
