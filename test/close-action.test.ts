import { readFileSync } from 'node:fs'
import { describe, expect, it } from 'vitest'
import {
  CLOSE_ACTION_DEFAULT,
  CLOSE_ACTION_OPTIONS,
  normalizeCloseAction,
} from '../src/utils/close-action'

describe('close action normalization', () => {
  it('defaults to hiding in tray', () => {
    expect(CLOSE_ACTION_DEFAULT).toBe('tray')
  })

  it('exposes exactly the tray and quit options', () => {
    expect(CLOSE_ACTION_OPTIONS).toEqual(['tray', 'quit'])
  })

  it('keeps the two supported literal values', () => {
    expect(normalizeCloseAction('tray')).toBe('tray')
    expect(normalizeCloseAction('quit')).toBe('quit')
  })

  it('falls back to tray for anything outside the whitelist', () => {
    expect(normalizeCloseAction(undefined)).toBe('tray')
    expect(normalizeCloseAction(null)).toBe('tray')
    expect(normalizeCloseAction('')).toBe('tray')
    expect(normalizeCloseAction('TRAY')).toBe('tray')
    expect(normalizeCloseAction('bogus')).toBe('tray')
    expect(normalizeCloseAction({})).toBe('tray')
  })
})

describe('config close action control contract', () => {
  it('invokes update_app_config once with a camelCase closeAction payload', () => {
    const source = readFileSync(new URL('../src/components/config-close-action.tsx', import.meta.url), 'utf8')

    expect(source).toContain('export function ConfigCloseAction')
    expect(source).toContain('invoke<AppConfig>(\'update_app_config\', { closeAction')
    // 受控值始终经归一化，未加载到配置时回落 tray 而不是给 HeroUI 传 undefined
    expect(source).toContain('selectedKey={normalizeCloseAction(')
  })

  it('surfaces failures as a danger toast without a success toast', () => {
    const source = readFileSync(new URL('../src/components/config-close-action.tsx', import.meta.url), 'utf8')

    expect(source).toContain('messages.close_action_failed')
    expect(source).toContain('variant: \'danger\'')
    expect(source).not.toContain('messages.close_action_saved')
  })

  it('stays within the shell conventions', () => {
    const source = readFileSync(new URL('../src/components/config-close-action.tsx', import.meta.url), 'utf8')

    expect(source).not.toContain('useCallback')
    expect(source).not.toContain('useMemo')
    expect(source).not.toContain('navigator.clipboard')
  })
})
