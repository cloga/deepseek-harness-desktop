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

  it('keeps callback CSP allowances isolated to the E2E build config', () => {
    const production = JSON.parse(
      readFileSync(new URL('../src-tauri/tauri.conf.json', import.meta.url), 'utf8'),
    )
    const e2e = JSON.parse(
      readFileSync(new URL('../src-tauri/tauri.e2e.conf.json', import.meta.url), 'utf8'),
    )
    expect(production.app.security.csp).not.toContain('http://127.0.0.1:3081')
    expect(production.app.security.csp).not.toContain('script-src \'self\' \'unsafe-inline\'')
    expect(e2e.app.security.csp).toContain('connect-src \'self\' ipc: http://ipc.localhost http://127.0.0.1:3081')
    expect(e2e.app.security.csp).toContain('script-src \'self\' \'unsafe-inline\'')
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

  it('records authenticated child execution and accepts one strict shell result', async () => {
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

    const callbackUrl = `http://127.0.0.1:${port}/e2e-shell-result`
    const missingOrigin = await fetch(callbackUrl, {
      method: 'POST',
      body: JSON.stringify({ value: { full: { width: 1 } } }),
    })
    expect(missingOrigin.status).toBe(403)
    const oversized = await fetch(callbackUrl, {
      method: 'POST',
      headers: { origin: 'http://tauri.localhost' },
      body: 'x'.repeat(64 * 1024 + 1),
    })
    expect(oversized.status).toBe(413)
    const invalidShape = await fetch(callbackUrl, {
      method: 'POST',
      headers: { origin: 'http://tauri.localhost' },
      body: JSON.stringify({ unexpected: true }),
    })
    expect(invalidShape.status).toBe(400)
    const expected = { value: { full: { width: 1024 }, restored: { width: 1024 } } }
    const accepted = await fetch(callbackUrl, {
      method: 'POST',
      headers: { origin: 'http://tauri.localhost' },
      body: JSON.stringify(expected),
    })
    expect(accepted.status).toBe(204)
    expect(accepted.headers.get('access-control-allow-origin')).toBe('http://tauri.localhost')
    expect(await (await fetch(callbackUrl)).json()).toEqual(expected)
    const duplicate = await fetch(callbackUrl, {
      method: 'POST',
      headers: { origin: 'http://tauri.localhost' },
      body: JSON.stringify(expected),
    })
    expect(duplicate.status).toBe(409)
    expect(output).toContain('E2E shell result rejected origin')
    expect(output).toContain('E2E shell result rejected size')
    expect(output).toContain('E2E shell result rejected shape')
    expect(output).toContain('E2E shell result accepted')
    expect(output).toContain('E2E shell result rejected duplicate')
  })

  it('accepts a structured shell scenario failure without logging its body', async () => {
    const port = await reservePort()
    const child = spawn(process.execPath, [fileURLToPath(fixture), '--port', String(port)], {
      stdio: ['ignore', 'pipe', 'pipe'],
    })
    fixtureProcesses.push(child)
    let output = ''
    child.stdout?.on('data', chunk => output += chunk.toString())
    await waitForFixture(port)
    const callbackUrl = `http://127.0.0.1:${port}/e2e-shell-result`
    const expected = { error: 'SENTINEL_SCENARIO_FAILURE' }
    const accepted = await fetch(callbackUrl, {
      method: 'POST',
      headers: { origin: 'http://tauri.localhost' },
      body: JSON.stringify(expected),
    })

    expect(accepted.status).toBe(204)
    expect(await (await fetch(callbackUrl)).json()).toEqual(expected)
    expect(output).not.toContain(expected.error)
    expect(output).toContain('E2E shell result accepted')
  })
})
