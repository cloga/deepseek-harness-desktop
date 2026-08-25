export interface ReadinessProbeResult {
  healthy: boolean
  notOwned: boolean
}

interface PollReadinessOptions {
  probe: () => Promise<ReadinessProbeResult>
  intervalMs: number
  maxAttempts?: number
  shouldContinue?: () => boolean
  wait?: (milliseconds: number) => Promise<void>
}

function delay(milliseconds: number): Promise<void> {
  return new Promise(resolve => setTimeout(resolve, milliseconds))
}

export async function pollReadiness({
  probe,
  intervalMs,
  maxAttempts,
  shouldContinue = () => true,
  wait = delay,
}: PollReadinessOptions): Promise<ReadinessProbeResult> {
  let remainingAttempts = maxAttempts

  while (shouldContinue() && remainingAttempts !== 0) {
    const result = await probe()
    if (remainingAttempts !== undefined) {
      remainingAttempts--
    }

    if (result.healthy || result.notOwned) {
      return result
    }
    if (shouldContinue() && remainingAttempts !== 0) {
      await wait(intervalMs)
    }
  }

  return { healthy: false, notOwned: false }
}
