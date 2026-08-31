import type { Event } from '@tauri-apps/api/event'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const eventListeners = new Map<string, (event: Event<unknown>) => void>()
const invoke = vi.fn()

vi.mock('@tauri-apps/api/core', () => ({ invoke }))
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async (event: string, callback: (payload: Event<unknown>) => void) => {
    eventListeners.set(event, callback)
    return vi.fn()
  }),
}))
vi.mock('@hairy/react-lib', () => ({ emitter: { emit: vi.fn() } }))
vi.mock('@/config/client', () => ({ queryClient: { invalidateQueries: vi.fn() } }))
vi.mock('../src/store/modules/harness-updater', () => ({
  harnessUpdater: { checkForUpdate: vi.fn() },
}))

const { harness } = await import('../src/store/modules/harness/store')

beforeEach(() => {
  eventListeners.clear()
  invoke.mockReset()
  Object.assign(harness, {
    status: 'ready',
    errorMsg: '',
    errorLogs: [],
    pluginConflictHint: '',
    inotifyLimitHint: '',
    serviceHealthy: true,
    serviceRunning: true,
    iframeLoaded: true,
    iframeError: true,
    busyAction: null,
    recovery: { required: false, info: null, attempts: 0, busy: false },
  })
})

describe('runtime exit store', () => {
  it('registers the dedicated backend event listener', async () => {
    await harness.listenProcessExit()
    expect(eventListeners.has('harness-process-exited')).toBe(true)
  })

  it('withdraws stale runtime state and exposes diagnostics after ownership is lost', async () => {
    invoke.mockImplementation(async (command: string) => {
      if (command === 'proxy_health_check')
        throw new Error('HARNESS_NOT_OWNED: no Harness process is owned by this app')
      if (command === 'read_service_logs')
        return 'Error: runtime process exited\nlast useful line'
      if (command === 'detect_plugin_recovery')
        return { plugins: [], reason: 'unknown', detail: '', raw_error: '' }
      throw new Error(`unexpected invoke: ${command}`)
    })

    await harness.handleProcessExit({ pid: 42, exitCode: 0 })

    expect(harness.status).toBe('error')
    expect(harness.serviceHealthy).toBe(false)
    expect(harness.serviceRunning).toBe(false)
    expect(harness.iframeLoaded).toBe(false)
    expect(harness.iframeError).toBe(false)
    expect(harness.errorLogs).toContain('Error: runtime process exited')
  })

  it('does not clobber a replacement process that owns the runtime slot', async () => {
    invoke.mockResolvedValue('HARNESS_NOT_READY: replacement process is starting')
    const original = {
      status: harness.status,
      serviceHealthy: harness.serviceHealthy,
      serviceRunning: harness.serviceRunning,
      iframeLoaded: harness.iframeLoaded,
      iframeError: harness.iframeError,
    }

    await harness.handleProcessExit({ pid: 41, exitCode: null })

    expect({
      status: harness.status,
      serviceHealthy: harness.serviceHealthy,
      serviceRunning: harness.serviceRunning,
      iframeLoaded: harness.iframeLoaded,
      iframeError: harness.iframeError,
    }).toEqual(original)
  })

  it('does not commit ready when the owned process exits during the final runtime-info read', async () => {
    let healthChecks = 0
    let runtimeInfoReads = 0
    let resolveFinalRuntimeInfo!: (value: { service_url: string }) => void
    const finalRuntimeInfo = new Promise<{ service_url: string }>((resolve) => {
      resolveFinalRuntimeInfo = resolve
    })

    invoke.mockImplementation(async (command: string) => {
      if (command === 'launch_harness')
        return 1
      if (command === 'get_runtime_info') {
        runtimeInfoReads++
        if (runtimeInfoReads === 1)
          return { service_url: 'http://127.0.0.1:31415' }
        return finalRuntimeInfo
      }
      if (command === 'prepare_harness_webview')
        return { url: 'http://127.0.0.1:31415', verify_auth: false, claim: null, probe_origin: null }
      if (command === 'proxy_health_check') {
        healthChecks++
        if (healthChecks <= 2)
          return 'Healthy'
        throw new Error('HARNESS_NOT_OWNED: process exited during readiness commit')
      }
      if (command === 'read_service_logs')
        return 'Error: process exited during readiness commit'
      if (command === 'detect_plugin_recovery')
        return { plugins: [], reason: 'unknown', detail: '', raw_error: '' }
      throw new Error(`unexpected invoke: ${command}`)
    })

    const launch = harness.launchAndWait()
    await vi.waitFor(() => expect(runtimeInfoReads).toBe(2))
    await harness.handleProcessExit({ pid: 43, exitCode: 1 })
    resolveFinalRuntimeInfo({ service_url: 'http://127.0.0.1:31415' })
    await launch

    expect(harness.status).toBe('error')
    expect(harness.serviceHealthy).toBe(false)
    expect(harness.serviceRunning).toBe(false)
    expect(harness.errorLogs).toContain('Error: process exited during readiness commit')
  })

  it('preserves a successful readiness poll across a transient owned recheck', async () => {
    let healthChecks = 0
    invoke.mockImplementation(async (command: string) => {
      if (command === 'launch_harness')
        return 2
      if (command === 'get_runtime_info')
        return { service_url: 'http://127.0.0.1:31415' }
      if (command === 'prepare_harness_webview')
        return { url: 'http://127.0.0.1:31415', verify_auth: false, claim: null, probe_origin: null }
      if (command === 'proxy_health_check') {
        healthChecks++
        return healthChecks === 1 ? 'Healthy' : 'HARNESS_NOT_READY: transient response'
      }
      throw new Error(`unexpected invoke: ${command}`)
    })

    await harness.launchAndWait()

    expect(harness.status).toBe('ready')
    expect(harness.serviceHealthy).toBe(true)
    expect(harness.serviceRunning).toBe(true)
  })

  it('clears running when the final ownership recheck detects an exit without an event', async () => {
    let healthChecks = 0
    invoke.mockImplementation(async (command: string) => {
      if (command === 'launch_harness')
        return 3
      if (command === 'get_runtime_info')
        return { service_url: 'http://127.0.0.1:31415' }
      if (command === 'proxy_health_check') {
        healthChecks++
        if (healthChecks === 1)
          return 'Healthy'
        throw new Error('HARNESS_NOT_OWNED: process exited before readiness commit')
      }
      if (command === 'read_service_logs')
        return 'Error: process exited before readiness commit'
      throw new Error(`unexpected invoke: ${command}`)
    })

    await expect(harness.launchAndWait()).rejects.toThrow()

    expect(harness.serviceHealthy).toBe(false)
    expect(harness.serviceRunning).toBe(false)
  })

  it('commits ready only after the protected iframe API probe succeeds', async () => {
    invoke.mockImplementation(async (command: string) => {
      if (command === 'launch_harness')
        return 4
      if (command === 'get_runtime_info')
        return { service_url: 'http://127.0.0.1:31415' }
      if (command === 'proxy_health_check')
        return 'Healthy'
      if (command === 'prepare_harness_webview') {
        return {
          url: 'http://127.0.0.1:31415/dsh-auth/4/42',
          verify_auth: true,
          claim: { generation: 4, pid: 42 },
          probe_origin: 'http://127.0.0.1:31415',
        }
      }
      if (command === 'finish_harness_webview_auth')
        return undefined
      if (command === 'show_harness_webview' || command === 'close_harness_webview')
        return undefined
      throw new Error(`unexpected invoke: ${command}`)
    })

    const launch = harness.launchAndWait()
    await vi.waitFor(() => expect(harness.iframeSrc).toContain('http://127.0.0.1:31415/'))
    expect(harness.status).toBe('ready')
    harness.reportIframeAuthProbe('http://127.0.0.1:31415', 200)
    await launch

    expect(harness.serviceHealthy).toBe(true)
    expect(harness.serviceRunning).toBe(true)
  })

  it('releases a failed iframe auth probe so the same process can retry', async () => {
    let preparations = 0
    const finalized: boolean[] = []
    invoke.mockImplementation(async (command: string, args?: { success?: boolean }) => {
      if (command === 'launch_harness')
        return 5
      if (command === 'get_runtime_info')
        return { service_url: 'http://127.0.0.1:31415' }
      if (command === 'proxy_health_check')
        return 'Healthy'
      if (command === 'prepare_harness_webview') {
        preparations++
        return {
          url: `http://127.0.0.1:3141${preparations}/dsh-auth/5/43`,
          verify_auth: true,
          claim: { generation: 5, pid: 43 },
          probe_origin: `http://127.0.0.1:3141${preparations}`,
        }
      }
      if (command === 'finish_harness_webview_auth') {
        finalized.push(args?.success === true)
        return undefined
      }
      if (command === 'show_harness_webview' || command === 'close_harness_webview')
        return undefined
      if (command === 'read_service_logs')
        return ''
      throw new Error(`unexpected invoke: ${command}`)
    })

    const first = harness.launchAndWait()
    await vi.waitFor(() => expect(preparations).toBe(1))
    harness.reportIframeAuthProbe('http://127.0.0.1:31411', 401)
    await expect(first).rejects.toThrow('HARNESS_IFRAME_AUTH_REJECTED')

    const retry = harness.launchAndWait()
    await vi.waitFor(() => expect(preparations).toBe(2))
    harness.reportIframeAuthProbe('http://127.0.0.1:31412', 200)
    await retry

    expect(finalized).toEqual([false, true])
    expect(harness.serviceHealthy).toBe(true)
  })
})
