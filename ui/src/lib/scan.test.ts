import { describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))

import {
  busyPingOf,
  defaultRepeats,
  earlierTests,
  estimateScan,
  rankScan,
  MAX_CUSTOM_COMBOS,
  scanBands,
  suggestCombos,
  summarizeScan,
  swingsWidely,
  workedBands,
} from './scan.svelte'
import { resultFixture } from './speedtest.fixtures'
import type { ScanFile } from './bindings/ScanFile'
import type { ScanResult } from './bindings/ScanResult'

function r(name: string, o: Partial<ScanResult> = {}): ScanResult {
  return {
    atMs: 0,
    bands: name,
    isStart: false,
    outcome: 'measured',
    note: null,
    lteBand: 3,
    ltePci: 103,
    nrBand: null,
    got5g: false,
    lteRsrp: null,
    lteSinr: null,
    nrSinr: null,
    downloadMbps: 50,
    uploadMbps: 5,
    idlePingMs: 40,
    pingDownMs: 200,
    pingUpMs: 300,
    bytesUsed: 0,
    tests: 1,
    downloadMinMbps: null,
    downloadMaxMbps: null,
    uploadMinMbps: null,
    uploadMaxMbps: null,
    ...o,
  }
}
const names = (list: ScanResult[]) => list.map((x) => x.bands)

describe('rankScan order', () => {
  const fastDown = r('down', { downloadMbps: 200, uploadMbps: 1, pingDownMs: 900, pingUpMs: 1500 })
  const fastUp = r('up', { downloadMbps: 40, uploadMbps: 30, pingDownMs: 150, pingUpMs: 200 })
  const lowPing = r('ping', { downloadMbps: 30, uploadMbps: 5, pingDownMs: 60, pingUpMs: 80 })
  const dead = r('dead', { outcome: 'noService', downloadMbps: null })
  const list = [dead, fastDown, fastUp, lowPing]
  const file = (results: ScanResult[]) => ({ queue: [], results, startedMs: 0, start: { lteBands: '5' } }) as unknown as ScanFile
  const order = (results: ScanResult[], p: 'balanced' | 'download' | 'upload' | 'ping') =>
    rankScan(file(results), [], p, 'cloudflare', 1).rows.map((x) => x.bands)

  it('follows the chosen priority, with unmeasured configs last', () => {
    expect(order(list, 'download')).toEqual(['down', 'up', 'ping', 'dead'])
    expect(order(list, 'upload')).toEqual(['up', 'ping', 'down', 'dead'])
    expect(order(list, 'ping')).toEqual(['ping', 'up', 'down', 'dead'])
  })

  it('balanced does not reward a fast download on a stalled upload', () => {
    expect(order(list, 'balanced')[0]).toBe('up')
    expect(order(list, 'balanced').at(-1)).toBe('dead')
  })

  it('leaves out a test that failed from the ranking', () => {
    const failed = r('failed', { outcome: 'testFailed', downloadMbps: 999 })
    expect(order([failed, lowPing], 'download')[0]).toBe('ping')
  })

  it('uses the slower ping under load', () => {
    expect(busyPingOf(r('x', { pingDownMs: 100, pingUpMs: 900 }))).toBe(900)
    expect(busyPingOf(r('x', { pingDownMs: null, pingUpMs: null }))).toBeNull()
  })
})

describe('rankScan pick', () => {
  const NOW = 10_000_000
  const B1 = '1'
  const B3 = '4'
  const B1B3 = '5'
  const file = (o: Partial<ScanFile>): ScanFile =>
    ({ queue: [], results: [], startedMs: NOW - 1000, start: { lteBands: B1B3 } }) as unknown as ScanFile & typeof o
  const scanned = (results: ScanResult[], queue = results.map((x) => x.bands)) => ({ ...file({}), results, queue })
  const earlier = (band: number, o = {}) =>
    resultFixture({
      atMs: NOW - 60_000,
      provider: 'ookla',
      downloadMbps: 116,
      uploadMbps: 2.8,
      serving: { ...resultFixture().serving, lteBand: band },
      ...o,
    })
  const go = (f: ScanFile, history = [] as ReturnType<typeof resultFixture>[]) =>
    rankScan(f, history, 'balanced', 'ookla', NOW).pick

  const noFive = { got5g: false, nrBand: null, downloadMbps: 19.2, uploadMbps: 1.1 }
  const failed = { outcome: 'testFailed', downloadMbps: null, uploadMbps: null, got5g: true } as const

  it('never recommends a band set because the others have no speed result', () => {
    expect(go(scanned([r(B1B3, failed), r(B1, noFive)]))).toBeNull()
  })

  it('says not enough when one band set has numbers and the rest of the scan never ran', () => {
    expect(go(scanned([r(B1, { ...noFive, got5g: true })], [B1, B3]))).toBeNull()
  })

  it('folds in recent tests of the same band set, so the real best wins', () => {
    const f = scanned([r(B1B3, failed), r(B1, noFive)], [B1B3, B1, B3])
    const pick = go(f, [earlier(3), earlier(3, { allowedBands: B1B3, downloadMbps: 95 })].slice(0, 1))
    // B1+B3 still has no numbers, but the winner has 5G: it can't lose on 5G any more.
    expect(pick?.bands).toBe(B3)
    expect(pick?.measuredNow).toBe(false)
    expect(pick?.got5g).toBe(true)
    expect(pick?.earlier).toBe(1)
  })

  it('averages this scan and earlier tests, each test counting once', () => {
    const f = scanned([r(B3, { tests: 1, downloadMbps: 100, uploadMbps: 2, got5g: true })])
    const pick = go(f, [earlier(3, { downloadMbps: 200, uploadMbps: 4 })])
    expect(pick?.downloadMbps).toBe(150)
    expect(pick?.uploadMbps).toBe(3)
    expect(pick?.measuredNow).toBe(true)
  })

  it('prefers 5G over a faster band set without it', () => {
    const f = scanned([r(B1, { downloadMbps: 300, uploadMbps: 30, got5g: false }), r(B3, { downloadMbps: 50, uploadMbps: 2, got5g: true })])
    expect(go(f)?.bands).toBe(B3)
  })

  it('a band set this scan found dead stays out, whatever it did before', () => {
    const f = scanned([r(B3, { outcome: 'noService', downloadMbps: null }), r(B1, { ...noFive, got5g: true })])
    expect(go(f, [earlier(3)])?.bands).toBe(B1)
  })

  it('recommends the only band set with a cell when the others have no service', () => {
    const f = scanned([r(B3, { outcome: 'noService', downloadMbps: null }), r(B1, noFive)])
    expect(go(f)?.bands).toBe(B1)
  })

  it('keeps the lowest and highest download, from repeats and earlier tests', () => {
    const f = scanned([r(B3, { tests: 2, downloadMbps: 100, downloadMinMbps: 80, downloadMaxMbps: 120, uploadMbps: 2, got5g: true })])
    const pick = go(f, [earlier(3, { downloadMbps: 30, uploadMbps: 4 })])
    expect(pick?.lowMbps).toBe(30)
    expect(pick?.highMbps).toBe(120)
    expect(pick && swingsWidely(pick)).toBe(true)
  })

  it('does not call a small difference a swing', () => {
    const pick = go(scanned([r(B3, { tests: 2, downloadMbps: 100, downloadMinMbps: 90, downloadMaxMbps: 110, uploadMbps: 2, got5g: true })]))
    expect(pick && swingsWidely(pick)).toBe(false)
  })

  it('the pick is always the first row, with the same numbers, even when history changes the order', () => {
    // Today B3 beats B1+B3, but over 7 days B1+B3 is ahead. The user saw 69.0 in the summary and 32.5 first-ranked B3 in the table.
    const f = scanned([
      r(B1B3, { tests: 2, downloadMbps: 32.5, uploadMbps: 2, got5g: true, nrBand: 41 }),
      r(B3, { tests: 2, downloadMbps: 40, uploadMbps: 2, got5g: true, nrBand: 41 }),
    ])
    const history = [earlier(3, { allowedBands: B1B3, downloadMbps: 142, uploadMbps: 2 })]
    const { rows, pick } = rankScan(f, history, 'download', 'ookla', NOW)
    expect(rows.map((x) => x.bands)).toEqual([B1B3, B3])
    expect(pick).toBe(rows[0].numbers)
    expect(pick?.downloadMbps).toBeCloseTo((32.5 * 2 + 142) / 3)
    expect(pick?.tests).toBe(3)
    expect(rows[0].result?.downloadMbps).toBe(32.5)
  })

  it('lists a band set this scan did not reach only when it has earlier numbers', () => {
    const f = scanned([r(B1, { got5g: true })], [B1, B3, B1B3])
    const { rows } = rankScan(f, [earlier(3)], 'balanced', 'ookla', NOW)
    expect(rows.map((x) => x.bands).sort()).toEqual([B1, B3].sort())
    expect(rows.find((x) => x.bands === B3)?.result).toBeNull()
  })

  it('has nothing to say with no numbers at all', () => {
    expect(go(scanned([r(B1, failed)]))).toBeNull()
  })
})

describe('earlierTests', () => {
  const NOW = 10_000_000
  const test = (o = {}) => resultFixture({ atMs: NOW - 1000, provider: 'ookla', ...o })
  const count = (bands: string, h: ReturnType<typeof resultFixture>[]) => earlierTests(bands, h, 'ookla', NOW, NOW).length

  it('needs the same band set when it was recorded, and the serving band for older tests', () => {
    expect(count('4', [test()])).toBe(1)
    expect(count('4', [test({ allowedBands: '5' })])).toBe(0)
    expect(count('5', [test({ allowedBands: '5' })])).toBe(1)
    expect(count('5', [test()])).toBe(0)
  })

  it('skips invalid, old, unfinished, other-provider and this-scan tests', () => {
    expect(count('4', [test({ invalidReason: 'bandChange' })])).toBe(0)
    expect(count('4', [test({ atMs: NOW - 8 * 24 * 3600 * 1000 })])).toBe(0)
    expect(count('4', [test({ uploadMbps: null })])).toBe(0)
    expect(count('4', [test({ provider: 'cloudflare' })])).toBe(0)
    expect(count('4', [test({ atMs: NOW })])).toBe(0)
  })
})

describe('summarizeScan', () => {
  it('only warns about losing 5G when the best has it, and a refused band is not "no service"', () => {
    const best = r('4', { got5g: true, nrBand: 41 })
    const no5g = r('80000')
    const dead = r('40', { outcome: 'noService', downloadMbps: null })
    const refused = r('1', { outcome: 'failed', downloadMbps: null })
    const pick = { bands: '4', downloadMbps: 50, uploadMbps: 5, busyPingMs: 300, got5g: true, nrBand: 41, measuredNow: true, earlier: 0, tests: 1, lowMbps: 50, highMbps: 50 }
    const s = summarizeScan([best, no5g, dead, refused], pick)
    expect(names(s.lose5g)).toEqual(['80000'])
    expect(names(s.noService)).toEqual(['40'])
    expect(names(s.inconclusive)).toEqual(['1'])
    expect(s.bandsTried).toBe(3)
    expect(summarizeScan([no5g, dead], { ...pick, bands: '80000', got5g: false }).lose5g).toEqual([])
  })

  it('reports the fewest tests behind any speed', () => {
    expect(summarizeScan([r('4', { tests: 3 }), r('1', { tests: 2 })], null).tests).toBe(2)
  })
})

describe('scanBands', () => {
  it('takes the bands tried alone, not combinations', () => {
    expect(scanBands(['5', '1', '4', '5'])).toEqual([1, 3])
    expect(scanBands(['5', '1'], [32])).toEqual([1, 32])
  })
})

describe('suggestCombos', () => {
  it('gives every pair, then every triple, of up to 4 bands in band order', () => {
    expect(suggestCombos([3, 1])).toEqual([[1, 3]])
    expect(suggestCombos([20, 3, 1])).toEqual([[1, 3], [1, 20], [3, 20], [1, 3, 20]])
    const many = suggestCombos([1, 3, 7, 20, 28])
    expect(many).toHaveLength(6 + 4)
    expect(many.every((c) => !c.includes(28))).toBe(true)
    expect(many.slice(0, 6).every((c) => c.length === 2)).toBe(true)
  })
  it('never uses a downlink-only band, and leaves out the current set', () => {
    expect(suggestCombos([3, 32])).toEqual([])
    expect(suggestCombos([1, 3, 20], '5')).toEqual([[1, 20], [3, 20], [1, 3, 20]])
  })
  it('gives nothing for fewer than 2 bands', () => {
    expect(suggestCombos([])).toEqual([])
    expect(suggestCombos([3])).toEqual([])
  })
  it('keeps the cap the same as Rust', () => {
    // Rust: router_core::scan::MAX_CUSTOM_SETS
    expect(MAX_CUSTOM_COMBOS).toBe(12)
  })
})

describe('workedBands', () => {
  const file = (results: ScanResult[]) => ({ results }) as unknown as ScanFile
  const supported = [1, 3, 7, 20, 28, 32, 41]
  it('takes the scan results first, fastest first, then good and slow past results', () => {
    const scanned = file([
      r('1', { downloadMbps: 20 }),
      r('4', { downloadMbps: 90 }),
      r('5', { downloadMbps: 500 }),
      r('40', { outcome: 'noService', downloadMbps: null }),
    ])
    const tones = new Map([
      [41, 'slow' as const],
      [20, 'good' as const],
      [28, 'good' as const],
      [32, 'good' as const],
    ])
    expect(workedBands(scanned, tones, supported)).toEqual([3, 1, 20, 28])
    expect(workedBands(null, tones, supported)).toEqual([20, 28, 41])
  })
  it('skips unsupported and downlink-only bands', () => {
    expect(workedBands(file([r('80000000'), r('40000')]), new Map(), [1, 3])).toEqual([])
  })
})

describe('defaultRepeats', () => {
  it('runs 2 tests per config, and 3 for Thorough', () => {
    expect(defaultRepeats('quick')).toBe(2)
    expect(defaultRepeats('thorough')).toBe(3)
  })
})

describe('estimateScan', () => {
  const cf = { provider: 'cloudflare', size: 'light', ooklaTermsAccepted: false } as const

  it('counts the current set and each band for quick', () => {
    const e = estimateScan('quick', 3, cf)
    expect(e.configs).toBe(4)
    expect(e.megabytes).toBe(160)
    expect(e.minutes).toBe(9)
  })

  it('thorough adds at most 6 pairs and 4 triples', () => {
    expect(estimateScan('thorough', 2, cf).configs).toBe(1 + 2 + 1)
    expect(estimateScan('thorough', 2, cf, 1, 1).configs).toBe(1 + 2 + 1 + 2)
    expect(estimateScan('quick', 2, cf, 1, 1).configs).toBe(1 + 2)
    expect(estimateScan('thorough', 3, cf).configs).toBe(1 + 3 + 4)
    expect(estimateScan('thorough', 8, cf).configs).toBe(1 + 8 + 10)
  })

  it('uses the test size, and Ookla whatever size is set', () => {
    expect(estimateScan('quick', 1, { ...cf, size: 'full' }).megabytes).toBe(400)
    const ookla = { provider: 'ookla', size: 'light', ooklaTermsAccepted: true } as const
    expect(estimateScan('quick', 1, ookla).megabytes).toBe(300)
  })

  it('counts the combinations the user asked for in both modes', () => {
    expect(estimateScan('quick', 2, cf, 1, 0, 3).configs).toBe(1 + 2 + 3)
    expect(estimateScan('thorough', 2, cf, 1, 0, 3).configs).toBe(1 + 2 + 3 + 1)
    // A combinations-only scan still tests the current set first.
    expect(estimateScan('quick', 0, cf, 2, 0, 2).configs).toBe(3)
  })

  it('counts every test when a config is tested more than once', () => {
    expect(estimateScan('quick', 3, cf, 3).megabytes).toBe(480)
    expect(estimateScan('quick', 3, cf, 3).minutes).toBeGreaterThan(estimateScan('quick', 3, cf).minutes)
  })
})
