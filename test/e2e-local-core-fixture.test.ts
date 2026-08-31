import { spawnSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
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

function fnv1a(bytes: Uint8Array): string {
  let hash = 0xCBF2_9CE4_8422_2325n
  for (const byte of bytes)
    hash = BigInt.asUintN(64, (hash ^ BigInt(byte)) * 0x100_0000_01B3n)
  return hash.toString(16).padStart(16, '0')
}

afterEach(() => {
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
})
