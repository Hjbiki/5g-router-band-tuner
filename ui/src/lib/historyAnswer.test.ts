import { describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))

import { ANSWER_MIN_TESTS, CLEAR_LEAD, answerFor, dayPart } from './historyAnswer'
import { resultFixture } from './speedtest.fixtures'
import type { SpeedTestResult } from './bindings/SpeedTestResult'

/** `n` valid tests on the band set `hex`, all at `hour` local time. */
function tests(hex: string, downs: number[], o: Partial<SpeedTestResult> = {}, hour = 20): SpeedTestResult[] {
  return downs.map((downloadMbps, i) =>
    resultFixture({ atMs: new Date(2026, 10, 2 + i, hour).getTime(), allowedBands: hex, downloadMbps, uploadMbps: 10, ...o }),
  )
}

describe('answerFor', () => {
  it('a: says there is nothing to compare without a valid test', () => {
    expect(answerFor([])).toEqual({ kind: 'noTests' })
    expect(answerFor(tests('4', [100], { invalidReason: 'bandChange' }))).toEqual({ kind: 'noTests' })
  })

  it('b: waits for enough tests on at least two band sets', () => {
    const a = answerFor([...tests('4', [100, 110, 120]), ...tests('5', [60, 70])])
    expect(a).toMatchObject({ kind: 'fewTests', provider: 'cloudflare', tests: 5 })
    expect(ANSWER_MIN_TESTS).toBe(3)
  })

  it('c: says when every test ran on one band set', () => {
    const a = answerFor(tests('4', [100, 120]))
    expect(a.kind).toBe('oneSet')
    if (a.kind !== 'oneSet') return
    expect(a.row.bands).toBe('4')
    expect(a.row.medianDownMbps).toBe(110)
    expect(a.tests).toBe(2)
  })

  it('d: names the fastest set when it leads the next by the clear lead, with its part of the day', () => {
    const fast = [
      ...tests('4', [150, 160], {}, 20),
      ...tests('4', [140], {}, 9).map((r) => ({ ...r, atMs: r.atMs + 30 * 86_400_000 })),
    ]
    const a = answerFor([...fast, ...tests('5', [100, 110, 120]), ...tests('1', [10])])
    expect(a.kind).toBe('fastest')
    if (a.kind !== 'fastest') return
    expect(a.row.bands).toBe('4')
    expect(a.next.bands).toBe('5')
    expect(a.leadPct).toBe(36)
    expect(a.dayPart).toBe('evening')
    expect(a.tests).toBe(7)
  })

  it('d: has no part of the day when the fastest set ran in one hour only', () => {
    const a = answerFor([...tests('4', [150, 160, 170]), ...tests('5', [100, 110, 120])])
    expect(a).toMatchObject({ kind: 'fastest', dayPart: null })
  })

  it('e: calls the two fastest too close when they are less than the clear lead apart', () => {
    const a = answerFor([...tests('4', [115, 118, 119]), ...tests('5', [100, 100, 100])])
    expect(a.kind).toBe('tooClose')
    if (a.kind !== 'tooClose') return
    expect(a.a.bands).toBe('4')
    expect(a.b.bands).toBe('5')
    expect(CLEAR_LEAD).toBe(0.2)
  })

  it('reads the provider with the most tests, and never mixes providers', () => {
    const ookla = tests('4', [300, 300, 300], { provider: 'ookla' })
    const cf = tests('5', [100, 100])
    expect(answerFor([...ookla, ...cf])).toMatchObject({ kind: 'oneSet', provider: 'ookla' })
    expect(answerFor([...ookla, ...cf], 'cloudflare')).toMatchObject({ kind: 'oneSet', provider: 'cloudflare' })
  })
})

describe('dayPart', () => {
  it('splits the day in four', () => {
    expect([0, 5, 6, 11, 12, 17, 18, 23].map(dayPart)).toEqual([
      'night',
      'night',
      'morning',
      'morning',
      'afternoon',
      'afternoon',
      'evening',
      'evening',
    ])
  })
})
