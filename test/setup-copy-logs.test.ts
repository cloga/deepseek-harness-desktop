import { readFileSync } from 'node:fs'
import { describe, expect, it } from 'vitest'

describe('setup error page copy-logs contract (SYST-04)', () => {
  it('exposes a copyLogsHandler that fetches run logs via the Tauri command', () => {
    const source = readFileSync(new URL('../src/layout/components/setup.tsx', import.meta.url), 'utf8')
    expect(source).toContain('copyLogsHandler')
    expect(source).toContain(`invoke<string>('read_run_logs')`)
  })

  it('routes the logs through the native clipboard helper, never navigator.clipboard', () => {
    const source = readFileSync(new URL('../src/layout/components/setup.tsx', import.meta.url), 'utf8')
    expect(source).not.toContain('navigator.clipboard.writeText')
    expect(source).toContain('writeClipboardText(')
  })

  it('surfaces copy success with the shared logs_copied toast', () => {
    const source = readFileSync(new URL('../src/layout/components/setup.tsx', import.meta.url), 'utf8')
    expect(source).toContain('messages.logs_copied')
  })

  it('surfaces copy failure with the danger logs_copy_failed toast', () => {
    const source = readFileSync(new URL('../src/layout/components/setup.tsx', import.meta.url), 'utf8')
    const nativeWrite = source.indexOf('writeClipboardText(')
    expect(nativeWrite).toBeGreaterThan(-1)
    const failurePath = source.slice(nativeWrite, nativeWrite + 500)
    expect(failurePath).toContain('catch (err)')
    expect(failurePath).toContain('messages.logs_copy_failed')
    expect(failurePath).toContain('variant: \'danger\'')
  })

  it('renders a ghost button labelled with the copy_logs i18n key', () => {
    const source = readFileSync(new URL('../src/layout/components/setup.tsx', import.meta.url), 'utf8')
    expect(source).toContain('button({ tone: \'ghost\'')
    expect(source).toContain('buttons.copy_logs')
  })

  it('decorates the button with the Copy icon from the project icon set', () => {
    const source = readFileSync(new URL('../src/layout/components/setup.tsx', import.meta.url), 'utf8')
    expect(source).toContain('Copy')
    expect(source).toContain('@gravity-ui/icons')
  })
})
