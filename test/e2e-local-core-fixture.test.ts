import { spawn, spawnSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { createServer } from 'node:net'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { afterEach, describe, expect, it } from 'vitest'

const fixture = new URL(
  './e2e/local-core/node_modules/@deepseek-ai/dsh/lib/bin.js',
  import.meta.url,
)
const pluginNames = [
  'dsh-tauri',
  'dsh-tauri-ui',
  'dsh-tauri-worktree',
  'dsh-tauri-panel',
  'dsh-tauri-panel-extension',
  'dsh-tauri-session',
  'dsh-tauri-rightclick',
]
const tempDirs: string[] = []
const fixtureProcesses: ReturnType<typeof spawn>[] = []

function fnv1a(bytes: Uint8Array): string {
  let hash = 0xCBF2_9CE4_8422_2325n
  for (const byte of bytes)
    hash = BigInt.asUintN(64, (hash ^ BigInt(byte)) * 0x100_0000_01B3n)
  return hash.toString(16).padStart(16, '0')
}

function reservePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const server = createServer()
    server.once('error', reject)
    server.listen(0, '127.0.0.1', () => {
      const address = server.address()
      if (typeof address === 'string' || address === null) {
        server.close()
        reject(new Error('fixture port was not assigned'))
        return
      }
      server.close(error => error ? reject(error) : resolve(address.port))
    })
  })
}

async function waitForFixture(port: number): Promise<void> {
  const deadline = Date.now() + 5_000
  while (Date.now() < deadline) {
    try {
      await fetch(`http://127.0.0.1:${port}/`)
      return
    }
    catch {
      await new Promise(resolve => setTimeout(resolve, 25))
    }
  }
  throw new Error('fixture server did not start')
}

async function waitForOutput(callback: () => boolean): Promise<void> {
  const deadline = Date.now() + 2_000
  while (Date.now() < deadline) {
    if (callback())
      return
    await new Promise(resolve => setTimeout(resolve, 25))
  }
  throw new Error('fixture output did not arrive')
}

afterEach(() => {
  for (const process of fixtureProcesses.splice(0))
    process.kill()
  for (const path of tempDirs.splice(0))
    rmSync(path, { recursive: true, force: true })
})

describe('local core E2E fixture', () => {
  it('does not execute WebDriver scripts while the child is authenticating', () => {
    const source = readFileSync(
      new URL('./e2e/native-webview.mjs', import.meta.url),
      'utf8',
    )
    const sessionCreated = source.indexOf('sessionId = session.sessionId')
    const probeEvidence = source.indexOf(
      '[harness-webview] protected probe completed: status=200',
      sessionCreated,
    )
    const firstElementPoll = source.indexOf('let surface')
    expect(sessionCreated).toBeGreaterThan(-1)
    expect(probeEvidence).toBeGreaterThan(sessionCreated)
    expect(firstElementPoll).toBeGreaterThan(probeEvidence)
  })

  it('keeps the authenticated child offscreen across a same-service remount', () => {
    const webview = readFileSync(
      new URL('../src/layout/components/webview.tsx', import.meta.url),
      'utf8',
    )
    const cleanup = webview
      .split('async function syncNativeWebview()')[1]
      ?.split('}, [iframeKey')[0]
    expect(cleanup).toContain('set_harness_webview_bounds')
    expect(cleanup).toContain('OFFSCREEN_NATIVE_WEBVIEW_BOUNDS')
    expect(cleanup).not.toContain('close_harness_webview')

    const store = readFileSync(
      new URL('../src/store/modules/harness/store.ts', import.meta.url),
      'utf8',
    )
    expect(store).toContain('failed to close native Harness WebView after startup failure')
  })

  it('places the debug store at the Tauri app-data root', () => {
    const workflow = readFileSync(
      new URL('../.github/workflows/ci.yml', import.meta.url),
      'utf8',
    )
    expect(workflow).toContain(
      'Copy-Item test\\e2e\\store.dat (Join-Path $appRoot \'.store.dev.dat\')',
    )
    expect(workflow).not.toContain(
      'Copy-Item test\\e2e\\store.dat (Join-Path $appData \'.store.dev.dat\')',
    )
  })

  it('links the E2E profile to the deployed plugin build output', () => {
    const workflow = readFileSync(
      new URL('../.github/workflows/ci.yml', import.meta.url),
      'utf8',
    )
    expect(workflow).toContain(
      'Resolve-Path \'src-tauri\\resources\\node_modules\'',
    )
    expect(workflow).toContain('Built plugin manifest missing:')
    expect(workflow).toContain('Built plugin entry missing:')
    expect(workflow).not.toContain(
      'Resolve-Path \'src-tauri\\target\\debug\\resources\\internal-plugins\'',
    )
  })

  it('keeps the completed preinstall baseline synchronized with bundled presets', () => {
    const store = JSON.parse(
      readFileSync(new URL('./e2e/store.dat', import.meta.url), 'utf8'),
    )
    const presets = readFileSync(
      new URL('../src-tauri/resources/preset-plugins.json', import.meta.url),
    )
    expect(store.setting).toMatchObject({
      active_core: 'local',
      active_profile: 'web',
      auto_start: true,
      installed: true,
      preinstall_done: true,
      preset_hash: fnv1a(presets),
    })
  })

  it('materializes the profile artifacts produced by plugin add', () => {
    const dshHome = mkdtempSync(join(tmpdir(), 'dsh-e2e-fixture-'))
    tempDirs.push(dshHome)
    const specs = pluginNames.map(name => `link:C:/desktop/resources/internal-plugins/${name}`)
    const result = spawnSync(
      process.execPath,
      [fileURLToPath(fixture), 'plugin', '--profile', 'web', 'add', ...specs],
      {
        encoding: 'utf8',
        env: { ...process.env, DSH_HOME: dshHome },
      },
    )

    expect(result.status, result.stderr).toBe(0)
    const profileDir = join(dshHome, 'profiles', 'web')
    const profileManifest = JSON.parse(readFileSync(join(profileDir, 'package.json'), 'utf8'))
    expect(profileManifest.dependencies).toEqual(Object.fromEntries(
      pluginNames.map((name, index) => [name, specs[index]]),
    ))
    expect(profileManifest.dsh.profile.bundles).toEqual(pluginNames)
    for (const name of pluginNames) {
      const manifest = JSON.parse(
        readFileSync(join(profileDir, 'node_modules', name, 'package.json'), 'utf8'),
      )
      expect(manifest).toMatchObject({ name, version: '0.0.0-e2e' })
    }
  })

  it('records child script execution only for an authenticated request', async () => {
    const port = await reservePort()
    const child = spawn(process.execPath, [fileURLToPath(fixture), '--port', String(port)], {
      stdio: ['ignore', 'pipe', 'pipe'],
    })
    fixtureProcesses.push(child)
    let output = ''
    child.stdout?.on('data', chunk => output += chunk.toString())
    await waitForFixture(port)

    const anonymous = await fetch(`http://127.0.0.1:${port}/e2e-rendered`, { method: 'POST' })
    expect(anonymous.status).toBe(401)
    expect(output).not.toContain('E2E fixture child script executed')

    const exchange = await fetch(`http://127.0.0.1:${port}/?token=E2E_ONE_SHOT_TOKEN`, {
      redirect: 'manual',
    })
    const cookie = exchange.headers.get('set-cookie')?.split(';')[0]
    expect(cookie).toBe('dsh-e2e=signed')
    if (cookie === undefined)
      throw new Error('fixture auth cookie missing')
    const authenticated = await fetch(`http://127.0.0.1:${port}/e2e-rendered`, {
      method: 'POST',
      headers: { cookie },
    })
    expect(authenticated.status).toBe(204)
    await waitForOutput(() => output.includes('E2E fixture child script executed'))
    expect(output).toContain('E2E fixture child script executed')
  })

  it('rejects a second use of the one-shot authentication token', async () => {
    const port = await reservePort()
    const child = spawn(process.execPath, [fileURLToPath(fixture), '--port', String(port)], {
      stdio: ['ignore', 'pipe', 'pipe'],
    })
    fixtureProcesses.push(child)
    let output = ''
    child.stdout?.on('data', chunk => output += chunk.toString())
    await waitForFixture(port)

    const tokenUrl = `http://127.0.0.1:${port}/?token=E2E_ONE_SHOT_TOKEN`
    const first = await fetch(tokenUrl, { redirect: 'manual' })
    const second = await fetch(tokenUrl, { redirect: 'manual' })

    expect(first.status).toBe(303)
    expect(first.headers.get('set-cookie')).toContain('dsh-e2e=signed')
    expect(second.status).toBe(401)
    expect(second.headers.get('set-cookie')).toBeNull()
    await waitForOutput(() => output.includes('E2E fixture rejected reused auth token'))
    expect(output.match(/E2E fixture auth exchange request/g)).toHaveLength(1)
  })
})
