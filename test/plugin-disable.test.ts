import { readFileSync } from 'node:fs'
import { describe, expect, it } from 'vitest'

describe('plugin disable/enable frontend contract', () => {
  it('config-plugin.tsx defines onDisable and onEnable handlers', () => {
    const source = readFileSync(new URL('../src/components/config-plugin.tsx', import.meta.url), 'utf8')

    expect(source).toContain('onDisable')
    expect(source).toContain('onEnable')
  })

  it('config-plugin.tsx consumes the disable/enable hook helpers', () => {
    const source = readFileSync(new URL('../src/components/config-plugin.tsx', import.meta.url), 'utf8')

    // 组件通过 hook 暴露的 disablePlugin/enablePlugin 调用（命令名在 hook 文件内），
    // 与 upgrade/remove 直接 invoke 的模式一致地封装到可复用 helper。
    expect(source).toContain('disablePlugin(')
    expect(source).toContain('enablePlugin(')
  })

  it('config-plugin.tsx renders a Disabled badge via plugins.disabled_badge', () => {
    const source = readFileSync(new URL('../src/components/config-plugin.tsx', import.meta.url), 'utf8')

    expect(source).toContain('plugins.disabled_badge')
  })

  it('config-plugin.tsx extends the busy state union with disable + enable', () => {
    const source = readFileSync(new URL('../src/components/config-plugin.tsx', import.meta.url), 'utf8')

    expect(source).toContain('\'update\' | \'remove\' | \'disable\' | \'enable\'')
  })

  it('config-plugin.tsx does NOT open a confirmation dialog for disable (reversible action)', () => {
    const source = readFileSync(new URL('../src/components/config-plugin.tsx', import.meta.url), 'utf8')

    // onDisable must not call openDialog — the disable handler is a direct action,
    // unlike onRemove which confirms via openDialog. We assert the onDisable block
    // contains no openDialog invocation.
    const onDisableMatch = source.match(/async function onDisable[\s\S]*?\n {2}\}/)
    expect(onDisableMatch).not.toBeNull()
    expect(onDisableMatch![0]).not.toContain('openDialog')
  })

  it('use-dsh-plugins.ts exposes disable_dsh_plugin and enable_dsh_plugin invoke calls', () => {
    const source = readFileSync(new URL('../src/hooks/use-dsh-plugins.ts', import.meta.url), 'utf8')

    expect(source).toContain('disable_dsh_plugin')
    expect(source).toContain('enable_dsh_plugin')
  })

  it('en-US.json contains all 7 disable/enable i18n keys', () => {
    const source = readFileSync(new URL('../src/i18n/locales/en-US.json', import.meta.url), 'utf8')

    expect(source).toContain('"plugins.disable"')
    expect(source).toContain('"plugins.enable"')
    expect(source).toContain('"plugins.disabled_badge"')
    expect(source).toContain('"plugins.disable_toast"')
    expect(source).toContain('"plugins.disable_failed"')
    expect(source).toContain('"plugins.enable_toast"')
    expect(source).toContain('"plugins.enable_failed"')
  })

  it('zh-CN.json contains all 7 disable/enable i18n keys', () => {
    const source = readFileSync(new URL('../src/i18n/locales/zh-CN.json', import.meta.url), 'utf8')

    expect(source).toContain('"plugins.disable"')
    expect(source).toContain('"plugins.enable"')
    expect(source).toContain('"plugins.disabled_badge"')
    expect(source).toContain('"plugins.disable_toast"')
    expect(source).toContain('"plugins.disable_failed"')
    expect(source).toContain('"plugins.enable_toast"')
    expect(source).toContain('"plugins.enable_failed"')
  })

  it('config-plugin.tsx stays within shell conventions (no useCallback/useMemo)', () => {
    const source = readFileSync(new URL('../src/components/config-plugin.tsx', import.meta.url), 'utf8')

    expect(source).not.toContain('useCallback')
    expect(source).not.toContain('useMemo')
  })
})
