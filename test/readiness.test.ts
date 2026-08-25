import { describe, expect, it } from 'vitest'
import { pollReadiness } from '../src/utils/readiness'

function noWait(): Promise<void> {
  return Promise.resolve()
}

describe('pollReadiness', () => {
  it('stops after the initial bounded attempts', async () => {
    let attempts = 0
    const result = await pollReadiness({
      probe: async () => {
        attempts++
        return { healthy: false, notOwned: false }
      },
      intervalMs: 0,
      maxAttempts: 3,
      wait: noWait,
    })

    expect(result).toEqual({ healthy: false, notOwned: false })
    expect(attempts).toBe(3)
  })

  it('keeps recovery polling until a slow service becomes healthy', async () => {
    let attempts = 0
    const result = await pollReadiness({
      probe: async () => {
        attempts++
        return { healthy: attempts === 5, notOwned: false }
      },
      intervalMs: 0,
      wait: noWait,
    })

    expect(result.healthy).toBe(true)
    expect(attempts).toBe(5)
  })

  it('stops recovery when the owning startup is superseded', async () => {
    let attempts = 0
    let active = true
    const result = await pollReadiness({
      probe: async () => {
        attempts++
        active = false
        return { healthy: false, notOwned: false }
      },
      intervalMs: 0,
      shouldContinue: () => active,
      wait: noWait,
    })

    expect(result).toEqual({ healthy: false, notOwned: false })
    expect(attempts).toBe(1)
  })

  it('discards a healthy probe result when recovery is cancelled while probing', async () => {
    let active = true
    const pendingProbe = Promise.withResolvers<{ healthy: boolean, notOwned: boolean }>()
    const resultPromise = pollReadiness({
      probe: () => pendingProbe.promise,
      intervalMs: 0,
      shouldContinue: () => active,
      wait: noWait,
    })

    active = false
    pendingProbe.resolve({ healthy: true, notOwned: false })

    await expect(resultPromise).resolves.toEqual({ healthy: false, notOwned: false })
  })
})
