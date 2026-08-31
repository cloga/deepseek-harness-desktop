/* eslint-disable react/dom-no-unsafe-iframe-sandbox */
import type { UnlistenFn } from '@tauri-apps/api/event'
import { CircleExclamation } from '@gravity-ui/icons'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { useEffect, useRef } from 'react'
import { useTranslation } from 'react-i18next'
import { If } from 'react-if-lite'
import { useStore } from 'valtio-define'
import { PluginRecovery } from '@/components/plugin-recovery'
import { useDesktopZoom } from '@/hooks/use-desktop-zoom'
import { useIframeShim } from '@/hooks/use-iframe-shim'
import { store } from '@/store'
import {
  fitNativeWebviewAroundOverlays,
  floatingOverlayBounds,
  hasBlockingOverlay,
  serializeNativeWebviewBounds,
  shouldShowNativeWebview,
  shouldSyncNativeWebviewBounds,
} from '@/utils/native-webview-visibility'
import { Loadable } from './loadable'
import { Navbar } from './navbar'
import { PreinstallSetup } from './preinstall-setup'
import { Setup } from './setup'

const STARTUP_STATUS_KEYS = {
  'plugin-install': 'status.loading_internal',
  'process-boot': 'status.loading_process',
  'client-modules': 'status.loading_client_modules',
} as const
const OFFSCREEN_NATIVE_WEBVIEW_BOUNDS = { x: -1, y: -1, width: 1, height: 1 }

let harnessAuthProbeListener: Promise<UnlistenFn> | undefined

function ensureHarnessAuthProbeListener() {
  harnessAuthProbeListener ??= listen<{ origin: string, status: number }>('harness-auth-probe', (event) => {
    store.harness.reportIframeAuthProbe(event.payload.origin, event.payload.status)
  })
  return harnessAuthProbeListener
}

/**
 * 主区域视图：壳层导航栏（Navbar）常驻顶部，
 * 安装/错误态渲染 Setup，就绪态渲染 iframe
 * （挂载后加载职责交给 dsh 应用内官方 boot 页，避免两套 loading 叠加）。
 * 状态与方法全部来自 harness store，不再接收 props。
 */
export function Webview() {
  const { t } = useTranslation()
  const {
    status,
    serviceHealthy,
    startupPhase,
    iframeError,
    iframeKey,
    iframeLoaded,
    iframeSrc,
    nativeWebview,
    nativeWebviewBounds,
    nativeWebviewMounted,
    nativeWebviewOrigin,
    serviceUrl,
    recovery,
  } = useStore(store.harness)

  const iframeRef = useRef<HTMLIFrameElement>(null)
  const nativeWebviewRef = useRef<HTMLDivElement>(null)

  useDesktopZoom(iframeRef)
  useIframeShim(iframeRef)
  useEffect(() => {
    function handleAuthProbe(event: MessageEvent) {
      if (event.source !== iframeRef.current?.contentWindow)
        return
      const payload = event.data as { type?: unknown, status?: unknown }
      if (payload?.type !== 'dsh://auth-probe' || typeof payload.status !== 'number')
        return
      store.harness.reportIframeAuthProbe(event.origin, payload.status)
    }
    window.addEventListener('message', handleAuthProbe)
    return () => window.removeEventListener('message', handleAuthProbe)
  }, [])
  useEffect(() => {
    let disposed = false
    let mounted = false
    let mounting = false

    async function syncNativeWebview() {
      const element = nativeWebviewRef.current
      if (disposed || !nativeWebview || !serviceHealthy || element === null)
        return
      const bounds = element.getBoundingClientRect()
      const geometry = serializeNativeWebviewBounds(bounds)
      if (mounted || mounting)
        return
      mounting = true
      try {
        await ensureHarnessAuthProbeListener()
        await invoke('mount_harness_webview', {
          ...geometry,
          url: iframeSrc,
          probeOrigin: nativeWebviewOrigin,
        })
        mounted = true
        store.harness.nativeWebviewMounted = true
      }
      catch (error) {
        console.error('[Harness] failed to mount native WebView:', error)
        store.harness.markIframeError()
      }
      finally {
        mounting = false
      }
    }

    if (nativeWebview && serviceHealthy) {
      void syncNativeWebview()
    }

    return () => {
      disposed = true
      if (mounted) {
        void invoke('set_harness_webview_bounds', OFFSCREEN_NATIVE_WEBVIEW_BOUNDS)
      }
      store.harness.nativeWebviewMounted = false
    }
  }, [iframeKey, iframeSrc, nativeWebview, nativeWebviewOrigin, serviceHealthy])
  useEffect(() => {
    // WebView2 初始导航完成前重设 child bounds 可能阻断 relay 请求；probe ready
    // 前保持 mount 的内容区 geometry，之后本 effect 会立即应用最新布局。
    if (!shouldSyncNativeWebviewBounds(nativeWebview, nativeWebviewMounted, iframeLoaded))
      return
    let lastState = ''
    let syncQueue = Promise.resolve()
    async function syncVisibility() {
      const element = nativeWebviewRef.current
      if (element === null)
        return
      const next = shouldShowNativeWebview(
        nativeWebview,
        serviceHealthy,
        iframeLoaded,
        hasBlockingOverlay(document),
      )
      const base = element.getBoundingClientRect()
      const bounds = next
        ? fitNativeWebviewAroundOverlays(base, floatingOverlayBounds(document))
        : OFFSCREEN_NATIVE_WEBVIEW_BOUNDS
      const serializedBounds = serializeNativeWebviewBounds(bounds)
      const state = JSON.stringify(serializedBounds)
      if (state === lastState)
        return
      lastState = state
      await invoke('set_harness_webview_bounds', serializedBounds)
      store.harness.nativeWebviewBounds = serializedBounds
    }
    function scheduleVisibilitySync() {
      syncQueue = syncQueue.then(syncVisibility).catch((error) => {
        console.error('[Harness] failed to synchronize native WebView visibility:', error)
      })
    }
    const mutationObserver = new MutationObserver(() => {
      scheduleVisibilitySync()
    })
    const resizeObserver = new ResizeObserver(() => {
      scheduleVisibilitySync()
    })
    mutationObserver.observe(document.body, { childList: true, subtree: true })
    if (nativeWebviewRef.current !== null)
      resizeObserver.observe(nativeWebviewRef.current)
    scheduleVisibilitySync()
    return () => {
      mutationObserver.disconnect()
      resizeObserver.disconnect()
    }
  }, [iframeLoaded, nativeWebview, nativeWebviewMounted, serviceHealthy])
  if (status === 'error') {
    return (
      <main className="relative flex min-h-0 flex-1 flex-col bg-canvas">
        <Navbar />
        <div className="min-h-0 flex-1">
          {/* 能定位到问题插件时展示全屏恢复页（卸除此插件并继续检测）；否则普通错误页 */}
          <If cond={recovery.required} else={<Setup />}>
            <PluginRecovery fullScreen />
          </If>
        </div>
      </main>
    )
  }

  // 预装插件引导：独立于安装/加载界面，渲染推荐插件列表与安装控制台
  if (status === 'preinstall') {
    return (
      <main className="relative flex min-h-0 w-full flex-col bg-canvas">
        <Navbar />
        <div className="min-h-0 flex-1">
          <PreinstallSetup />
        </div>
      </main>
    )
  }

  if (status !== 'ready') {
    return (
      <main className="relative flex min-h-0 w-full flex-col bg-canvas">
        <Navbar />
        <div className="min-h-0 flex-1">
          <Setup />
        </div>
      </main>
    )
  }

  return (
    <main className="relative flex min-h-0 flex-1 flex-col bg-canvas">
      <Navbar iframeRef={iframeRef} />

      {/* iframe 区域：加载失败时用覆盖层展示重试（iframe 保持挂载，重试复用） */}
      <div className="relative min-h-0 flex-1">
        <If
          cond={serviceHealthy}
          else={<Loadable subtitle={t(STARTUP_STATUS_KEYS[startupPhase])} />}
        >
          <If
            cond={nativeWebview}
            then={(
              <div
                key={iframeKey}
                ref={nativeWebviewRef}
                className="h-full w-full bg-load-bg"
                data-testid="harness-native-surface"
                data-loaded={iframeLoaded}
                data-mounted={nativeWebviewMounted}
                data-native-x={nativeWebviewBounds.x}
                data-native-y={nativeWebviewBounds.y}
                data-native-width={nativeWebviewBounds.width}
                data-native-height={nativeWebviewBounds.height}
              />
            )}
            else={(
              <iframe
                key={iframeKey}
                ref={iframeRef}
                className="block h-full w-full border-none bg-load-bg"
                src={iframeSrc}
                allow="accelerometer; ambient-light-sensor; autoplay; battery; camera; clipboard-read; clipboard-write; display-capture; document-domain; encrypted-media; fullscreen; gamepad; geolocation; gyroscope; hid; idle-detection; keyboard-map; magnetometer; microphone; midi; payment; picture-in-picture; publickey-credentials-get; screen-wake-lock; serial; speaker-selection; usb; web-share; xr-spatial-tracking"
                sandbox="allow-same-origin allow-scripts allow-popups allow-forms allow-modals allow-downloads allow-storage-access-by-user-activation"
                onLoad={store.harness.markIframeLoaded}
                onError={store.harness.markIframeError}
                title={t('app.open_editor')}
              />
            )}
          />
        </If>

        <If cond={serviceHealthy && iframeError}>
          <div className="absolute inset-0 z-[1]">
            <Loadable
              icon={CircleExclamation}
              title={t('ui.iframe_error')}
              errorMsg={t('ui.ensure_running', { url: serviceUrl })}
              onRetry={store.harness.refreshIframe}
            />
          </div>
        </If>
      </div>
    </main>
  )
}
