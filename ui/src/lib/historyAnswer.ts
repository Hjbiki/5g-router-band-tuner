// The History page's answer card: which band set has been fastest here, in one sentence. Pure, so
// the rules can be tested on their own. It uses the medians of `byBandSet`, the same rows as the
// Speed by bands table, so the card and the table can't disagree.
import type { SpeedTestProvider } from './bindings/SpeedTestProvider'
import type { SpeedTestResult } from './bindings/SpeedTestResult'
import { CLEAR_LEAD, byBandSet, speedProviders, type BandSetRow } from './history.svelte'

export { CLEAR_LEAD }

/** A band set needs this many valid tests before the answer card compares it. */
export const ANSWER_MIN_TESTS = 3

export type DayPart = 'morning' | 'afternoon' | 'evening' | 'night'

export type HistoryAnswer =
  /** No valid speed test in the range. */
  | { kind: 'noTests' }
  /** More than one band set, but fewer than two with enough tests to compare. */
  | { kind: 'fewTests'; provider: SpeedTestProvider; tests: number }
  /** Every valid test ran on the same band set. */
  | { kind: 'oneSet'; provider: SpeedTestProvider; row: BandSetRow; tests: number }
  /** One band set's median download leads the next by at least CLEAR_LEAD. */
  | {
      kind: 'fastest'
      provider: SpeedTestProvider
      row: BandSetRow
      next: BandSetRow
      /** How much faster, in whole percent. */
      leadPct: number
      /** When in the day the fastest set did best; null with too few tests to say. */
      dayPart: DayPart | null
      tests: number
    }
  /** The two fastest are less than CLEAR_LEAD apart. */
  | { kind: 'tooClose'; provider: SpeedTestProvider; a: BandSetRow; b: BandSetRow; tests: number }

/** The part of the day a local hour falls in. */
export function dayPart(hour: number): DayPart {
  if (hour < 6) return 'night'
  if (hour < 12) return 'morning'
  if (hour < 18) return 'afternoon'
  return 'evening'
}

const sumTests = (rows: readonly BandSetRow[]) => rows.reduce((n, r) => n + r.tests, 0)

/**
 * The answer for the tests in a range. Without `provider`, it reads the provider with the most
 * valid tests (Cloudflare on a tie), since speeds from two providers aren't comparable.
 */
export function answerFor(speeds: readonly SpeedTestResult[], provider?: SpeedTestProvider): HistoryAnswer {
  const providers = provider ? [provider] : speedProviders(speeds)
  let best: { provider: SpeedTestProvider; rows: BandSetRow[] } | null = null
  for (const p of providers) {
    const rows = byBandSet(speeds, p)
    if (rows.length && (!best || sumTests(rows) > sumTests(best.rows))) best = { provider: p, rows }
  }
  if (!best) return { kind: 'noTests' }
  const { rows } = best
  const tests = sumTests(rows)
  if (rows.length === 1) return { kind: 'oneSet', provider: best.provider, row: rows[0], tests }

  const eligible = rows
    .filter((r) => r.tests >= ANSWER_MIN_TESTS && r.medianDownMbps !== null)
    .sort((a, b) => (b.medianDownMbps as number) - (a.medianDownMbps as number))
  if (eligible.length < 2) return { kind: 'fewTests', provider: best.provider, tests }

  const [top, next] = eligible
  const topDown = top.medianDownMbps as number
  const nextDown = next.medianDownMbps as number
  if (topDown >= nextDown * (1 + CLEAR_LEAD)) {
    return {
      kind: 'fastest',
      provider: best.provider,
      row: top,
      next,
      leadPct: nextDown > 0 ? Math.round((topDown / nextDown - 1) * 100) : 100,
      dayPart: top.bestHour === null ? null : dayPart(top.bestHour),
      tests,
    }
  }
  return { kind: 'tooClose', provider: best.provider, a: top, b: next, tests }
}
