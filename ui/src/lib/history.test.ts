import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))
vi.mock('./backend', () => ({
  api: {
    historyOverview: vi.fn(),
    historySignal: vi.fn(),
    historySpeedtests: vi.fn(),
    historyEvents: vi.fn(),
    scanPeriods: vi.fn(),
  },
  onHistoryCleared: vi.fn(),
}))

import { api, onHistoryCleared } from './backend'
import { t } from './i18n'
import {
  bandRibbon,
  dataUseMb,
  byBandSet,
  byHourOfDay,
  ensureHistory,
  history,
  loadHistory,
  medianOf,
  metricValues,
  niceAxis,
  parseDay,
  rangeBounds,
  resetHistory,
  setRange,
  signalMarkers,
  sortBandSets,
  sourceShape,
  speedProviders,
  speedSource,
  signalHealth,
  validCustom,
  byIpRange,
  fasterRange,
  hasIpRanges,
  sections,
  sectionOpen,
  setSectionOpen,
  DEFAULT_RANGE,
} from './history.svelte'
import { overviewFixture, pointFixture, rebootEntry } from './history.fixtures'
import { resultFixture } from './speedtest.fixtures'

const H = 3_600_000
const D = 24 * H

describe('rangeBounds', () => {
  const now = new Date(2026, 10, 6, 15, 30).getTime()

  it('ends the fixed ranges now', () => {
    expect(rangeBounds({ kind: '24h' }, now)).toMatchObject({ fromMs: now - D, toMs: now })
    expect(rangeBounds({ kind: '7d' }, now)).toMatchObject({ fromMs: now - 7 * D, toMs: now })
    expect(rangeBounds({ kind: '30d' }, now)).toMatchObject({ fromMs: now - 30 * D, toMs: now })
  })

  it('sends the offset from UTC the way the backend reads it', () => {
    expect(rangeBounds({ kind: '24h' }, now).utcOffsetMin).toBe(-new Date(now).getTimezoneOffset())
  })

  it('runs a custom range from the first local midnight to the end of its last day', () => {
    const from = new Date(2026, 10, 1, 9).getTime()
    const to = new Date(2026, 10, 3).getTime()
    const r = rangeBounds({ kind: 'custom', fromMs: from, toMs: to }, now)
    expect(r.fromMs).toBe(new Date(2026, 10, 1).getTime())
    expect(r.toMs).toBe(new Date(2026, 10, 4).getTime() - 1)
  })

  it('accepts a one-day custom range and refuses a backwards one', () => {
    const day = new Date(2026, 10, 2).getTime()
    expect(validCustom(day, day)).toBe(true)
    expect(validCustom(day + D, day)).toBe(false)
    expect(validCustom(null, day)).toBe(false)
    expect(parseDay('2026-11-02')).toBe(day)
    expect(parseDay('')).toBeNull()
  })
})

describe('speedSource', () => {
  const periods: [number, number][] = [[1000, 2000]]

  it('keeps the recorded source', () => {
    expect(speedSource({ atMs: 1500, source: 'auto' }, periods)).toBe('auto')
    expect(speedSource({ atMs: 5000, source: 'scan' }, periods)).toBe('scan')
  })

  it('infers scan inside a scan period and manual elsewhere for older results', () => {
    expect(speedSource({ atMs: 1500 }, periods)).toBe('scan')
    expect(speedSource({ atMs: 2000, source: null }, periods)).toBe('scan')
    expect(speedSource({ atMs: 2500 }, periods)).toBe('manual')
    expect(speedSource({ atMs: 1500 }, [])).toBe('manual')
  })
})

describe('medianOf', () => {
  it('takes the middle value, or the mean of the middle two', () => {
    expect(medianOf([3, 1, 2])).toBe(2)
    expect(medianOf([4, 1, 3, 2])).toBe(2.5)
    expect(medianOf([null, undefined])).toBeNull()
  })
})

describe('signalHealth', () => {
  const MIN = 60_000
  it('weights service and 5G by time, and counts spells and restarts', () => {
    const points = [
      pointFixture({ tMs: 0, observedS: 60, serviceS: 60, nrShare: 1 }),
      // One spell over two buckets in a row.
      pointFixture({ tMs: MIN, observedS: 60, serviceS: 0, nrShare: null }),
      pointFixture({ tMs: 2 * MIN, observedS: 60, serviceS: 30, nrShare: 0 }),
      pointFixture({ tMs: 3 * MIN, observedS: 60, serviceS: 60, nrShare: 1 }),
      // A second spell after a gap.
      pointFixture({ tMs: 5 * MIN, observedS: 60, serviceS: 0, nrShare: null }),
    ]
    const s = signalHealth(points, { entries: [rebootEntry(10), rebootEntry(20)], nextBeforeMs: null }, MIN)
    expect(s.inServicePct).toBe(50)
    expect(s.on5gPct).toBeCloseTo((120 / 150) * 100, 5)
    expect(s.lost).toBe(2)
    expect(s.reboots).toBe(2)
    expect(s.rebootsMore).toBe(false)
  })

  it("doesn't count the app's own band changes and scans as lost service", () => {
    const points = [
      pointFixture({ tMs: 0, observedS: 60, serviceS: 60 }),
      pointFixture({ tMs: MIN, observedS: 60, serviceS: 0, ownChangeS: 60 }),
      pointFixture({ tMs: 2 * MIN, observedS: 60, serviceS: 50 }),
    ]
    const s = signalHealth(points, null, MIN)
    expect(s.lost).toBe(0)
    // 10 s lost of the 120 s that weren't the app's own change.
    expect(s.inServicePct).toBeCloseTo((110 / 120) * 100, 5)
  })

  it('has nothing to say without signal', () => {
    expect(signalHealth([], null)).toEqual({ inServicePct: null, on5gPct: null, lost: 0, reboots: 0, rebootsMore: false })
  })

  it('says when there may be more restarts than one page of events holds', () => {
    expect(signalHealth([], { entries: [rebootEntry(1)], nextBeforeMs: 0 }).rebootsMore).toBe(true)
  })
})

describe('byIpRange', () => {
  const r = (ipRange: string | null, downloadMbps: number, o = {}) => resultFixture({ ipRange, downloadMbps, uploadMbps: 10, atMs: downloadMbps, ...o })

  it('groups valid tests by range, compared ones first, and leaves unknown ranges last', () => {
    const rows = byIpRange(
      [
        r('10.45', 100),
        r('10.45', 120),
        r('10.46', 60),
        r('10.46', 80),
        r('10.47', 300),
        r(null, 50),
        r('10.45', 999, { invalidReason: 'ipChanged' }),
        r('10.45', 999, { provider: 'ookla' }),
        r('10.46', 999, { uploadMbps: null }),
      ],
      'cloudflare',
    )
    expect(rows.map((x) => [x.range, x.tests, x.compared])).toEqual([
      ['10.45', 2, true],
      ['10.46', 2, true],
      ['10.47', 1, false],
      [null, 1, false],
    ])
    expect(rows[0]).toMatchObject({ medianDownMbps: 110, medianUpMbps: 10, lastMs: 120 })
  })

  it('reads one provider at a time', () => {
    expect(byIpRange([r('10.45', 100, { provider: 'ookla' })], 'ookla')).toHaveLength(1)
    expect(byIpRange([r('10.45', 100, { provider: 'ookla' })], 'cloudflare')).toEqual([])
  })

  it('names a faster range only when it clearly leads the next compared one', () => {
    const lead = byIpRange([r('10.45', 130), r('10.45', 130), r('10.46', 100), r('10.46', 100)], 'cloudflare')
    expect(fasterRange(lead)).toBe('10.45')
    const close = byIpRange([r('10.45', 115), r('10.45', 115), r('10.46', 100), r('10.46', 100)], 'cloudflare')
    expect(fasterRange(close)).toBeNull()
    // One compared range and one with too few tests: nothing to set it beside.
    expect(fasterRange(byIpRange([r('10.45', 300), r('10.46', 100), r('10.46', 100)], 'cloudflare'))).toBeNull()
  })

  it('knows when no test recorded a range', () => {
    expect(hasIpRanges([r(null, 1)])).toBe(false)
    expect(hasIpRanges([r(null, 1), r('10.45', 1)])).toBe(true)
  })
})

describe('More details sections', () => {
  afterEach(() => {
    try {
      localStorage.clear()
    } catch {
      // jsdom always has it
    }
    resetHistory()
  })

  it('start closed, and remember being opened on this PC', () => {
    resetHistory()
    expect(sections.open).toEqual([])
    setSectionOpen('bandSets', true)
    setSectionOpen('events', true)
    setSectionOpen('events', false)
    expect(sectionOpen('bandSets')).toBe(true)
    expect(JSON.parse(localStorage.getItem('history.open')!)).toEqual(['bandSets'])
    resetHistory()
    expect(sections.open).toEqual(['bandSets'])
  })

  it('ignore anything unreadable in storage', () => {
    localStorage.setItem('history.open', '{"no"')
    resetHistory()
    expect(sections.open).toEqual([])
    localStorage.setItem('history.open', '["signal","nope"]')
    resetHistory()
    expect(sections.open).toEqual(['signal'])
  })

  it('a new user starts on the last 7 days, and a picked range is kept', () => {
    expect(DEFAULT_RANGE).toBe('7d')
    resetHistory()
    expect(history.range).toEqual({ kind: '7d' })
  })
})

describe('loadHistory', () => {
  beforeEach(() => {
    resetHistory()
    vi.useFakeTimers()
    vi.mocked(api.historyOverview).mockResolvedValue(overviewFixture())
    vi.mocked(api.historySignal).mockResolvedValue({ bucket: 'min5', bucketMs: 300_000, points: [] })
    vi.mocked(api.historySpeedtests).mockResolvedValue([])
    vi.mocked(api.historyEvents).mockResolvedValue({ entries: [], nextBeforeMs: null })
    vi.mocked(api.scanPeriods).mockResolvedValue([])
  })
  afterEach(() => {
    vi.useRealTimers()
    vi.clearAllMocks()
    try {
      localStorage.clear()
    } catch {
      // jsdom always has it
    }
  })

  it('waits for the range to settle, then asks once for the newest range', async () => {
    const now = new Date(2026, 10, 6, 12).getTime()
    vi.setSystemTime(now)
    void setRange({ kind: '7d' })
    const done = setRange({ kind: '30d' })
    expect(api.historyOverview).not.toHaveBeenCalled()
    await vi.advanceTimersByTimeAsync(200)
    await done
    expect(api.historyOverview).toHaveBeenCalledOnce()
    // The range is measured when the load runs, after the wait.
    const end = now + 200
    expect(api.historySignal).toHaveBeenCalledWith(expect.objectContaining({ fromMs: end - 30 * D, toMs: end }), 'auto')
    expect(api.historySpeedtests).toHaveBeenCalledWith(expect.objectContaining({ fromMs: end - 30 * D }))
    expect(history.loading).toBe(false)
    expect(JSON.parse(localStorage.getItem('history.range')!)).toEqual({ kind: '30d' })
  })

  it('drops an answer that arrives after a newer request', async () => {
    let first!: (v: ReturnType<typeof overviewFixture>) => void
    vi.mocked(api.historyOverview).mockImplementationOnce(() => new Promise((r) => (first = r)))
    void loadHistory(0)
    await vi.advanceTimersByTimeAsync(0)
    const second = loadHistory(0)
    await vi.advanceTimersByTimeAsync(0)
    await second
    expect(history.overview?.minutes).toBe(overviewFixture().minutes)
    first(overviewFixture({ minutes: 1 }))
    await vi.advanceTimersByTimeAsync(0)
    expect(history.overview?.minutes).toBe(overviewFixture().minutes)
  })

  it('skips the other queries when the database is unavailable', async () => {
    vi.mocked(api.historyOverview).mockResolvedValue(overviewFixture({ available: false, unavailableReason: 'broken' }))
    const done = loadHistory(0)
    await vi.advanceTimersByTimeAsync(0)
    await done
    expect(api.historySignal).not.toHaveBeenCalled()
    expect(history.overview?.available).toBe(false)
  })

  it('reloads when the history is cleared', async () => {
    let cleared: () => void = () => {}
    vi.mocked(onHistoryCleared).mockImplementation(async (cb) => {
      cleared = cb
      return () => {}
    })
    await ensureHistory()
    cleared()
    await vi.advanceTimersByTimeAsync(0)
    expect(api.historyOverview).toHaveBeenCalledOnce()
  })
})

describe('chart helpers', () => {
  it('turns band changes, restarts and app starts into markers, by how each change ended', () => {
    const item = (atMs: number, kind: string) => ({
      atMs,
      kind,
      category: 'app' as const,
      severity: 'info' as const,
      technical: false,
      key: 'events.kind.x',
      params: {},
      detailKey: null,
    })
    const group = (startedMs: number, outcome: { kind: 'kept' } | { kind: 'reverted'; reason: string; fallback: boolean } | { kind: 'rejected' }, to: number[]) => ({
      kind: 'change' as const,
      group: { startedMs, endedMs: startedMs + 1, from: [], to, primary: null, outcome, steps: [] },
    })
    const markers = signalMarkers({
      entries: [
        group(50, { kind: 'kept' }, [1, 3]),
        { kind: 'single', item: item(40, 'device.reboot') },
        group(30, { kind: 'reverted', reason: 'safeApply.reason.connectionLost', fallback: false }, [28]),
        group(20, { kind: 'rejected' }, []),
        { kind: 'single', item: item(10, 'app.started') },
        { kind: 'single', item: item(5, 'session.lost') },
      ],
      nextBeforeMs: null,
    })
    expect(markers.map((m) => [m.t, m.kind])).toEqual([
      [10, 'appStart'],
      [20, 'applied'],
      [30, 'reverted'],
      [40, 'reboot'],
      [50, 'kept'],
    ])
    expect(markers[4].label).toBe(t('history.marker.kept', { bands: 'B1 + B3' }))
    expect(markers[1].label).toBe(t('history.marker.applied', { bands: t('history.marker.otherBands') }))
  })

  it('joins buckets of the same allowed bands and breaks the ribbon at gaps', () => {
    const p = (m: number, allowedBands: string | null) => pointFixture({ tMs: m * 60_000, allowedBands })
    const spans = bandRibbon([p(0, '5'), p(1, '5'), p(2, '1'), p(5, '1'), p(6, null), p(7, '1')], 60_000)
    expect(spans).toEqual([
      { from: 0, to: 120_000, label: 'B1 + B3' },
      { from: 120_000, to: 180_000, label: 'B1' },
      { from: 300_000, to: 360_000, label: 'B1' },
      { from: 420_000, to: 480_000, label: 'B1' },
    ])
  })

  it('leaves 5G out of a metric when no bucket has a 5G value', () => {
    const pts = [pointFixture({ tMs: 0 }), pointFixture({ tMs: 60_000 })]
    expect(metricValues(pts, 'rsrq').nr).toBeNull()
    expect(metricValues(pts, 'sinr').nr?.[0]).toEqual({ t: 0, min: 14, avg: 16, max: 18 })
    expect(metricValues(pts, 'rsrp').lte[1]).toEqual({ t: 60_000, min: -97, avg: -95, max: -93 })
  })

  it('gives each source its own shape', () => {
    expect([sourceShape('manual'), sourceShape('scan'), sourceShape('auto')]).toEqual(['circle', 'square', 'triangle'])
  })
})

describe('speed aggregates', () => {
  const at = (hour: number, minute = 0, day = 2) => new Date(2026, 10, day, hour, minute).getTime()

  it('groups valid tests by the local hour they ran in', () => {
    const { hours, valid } = byHourOfDay([
      resultFixture({ atMs: at(8, 5), downloadMbps: 100 }),
      resultFixture({ atMs: at(8, 40, 3), downloadMbps: 50 }),
      resultFixture({ atMs: at(8, 50, 4), downloadMbps: 80 }),
      resultFixture({ atMs: at(21), downloadMbps: 30 }),
      resultFixture({ atMs: at(21, 30), downloadMbps: 900, invalidReason: 'bandChange' }),
      resultFixture({ atMs: at(22), downloadMbps: null }),
    ])
    expect(valid).toBe(4)
    expect(hours).toHaveLength(24)
    expect(hours[8]).toEqual({ hour: 8, n: 3, median: 80, p25: 65, p75: 90, few: false })
    expect(hours[21]).toMatchObject({ n: 1, median: 30, few: true })
    expect(hours[22]).toMatchObject({ n: 0, median: null, few: false })
  })

  it('measures upload and the slower ping under load too', () => {
    const r = resultFixture({ atMs: at(9), uploadMbps: 4, pingDownMs: 200, pingUpMs: 700 })
    expect(byHourOfDay([r], 'up').hours[9].median).toBe(4)
    expect(byHourOfDay([r], 'ping').hours[9].median).toBe(700)
  })

  it('groups band sets by the allowed bands, and older tests by the band that served them', () => {
    const serving = (lteBand: number, nrBand: number | null) => ({ ...resultFixture().serving, lteBand, nrBand })
    const rows = byBandSet(
      [
        resultFixture({ atMs: at(8), allowedBands: '5', downloadMbps: 100, uploadMbps: 10 }),
        resultFixture({ atMs: at(14), allowedBands: '0005', downloadMbps: 140, uploadMbps: 20, serving: serving(1, null) }),
        resultFixture({ atMs: at(14, 30), allowedBands: '5', downloadMbps: 120, uploadMbps: 30 }),
        resultFixture({ atMs: at(9), downloadMbps: 60, uploadMbps: 5 }),
        resultFixture({ atMs: at(10), allowedBands: '5', downloadMbps: 999, invalidReason: 'bandChange' }),
        resultFixture({ atMs: at(11), allowedBands: '5', uploadMbps: null }),
        resultFixture({ atMs: at(12), allowedBands: '5', provider: 'ookla', downloadMbps: 300 }),
      ],
      'cloudflare',
    )
    expect(rows).toHaveLength(2)
    const set = rows.find((r) => r.key === 'set:5')!
    expect(set).toMatchObject({ bands: '5', served: null, tests: 3, with5g: 2, medianDownMbps: 120, medianUpMbps: 20, lastMs: at(14, 30) })
    expect(set.bestHour).toBe(14)
    const served = rows.find((r) => r.key === 'served:3')!
    expect(served).toMatchObject({ bands: '4', served: 3, tests: 1, with5g: 1, bestHour: null })
    expect(served.medianBusyPingMs).toBe(1450)

    const ookla = byBandSet([resultFixture({ allowedBands: '5', provider: 'ookla', uploadMbps: 9 })], 'ookla')
    expect(ookla.map((r) => r.key)).toEqual(['set:5'])
  })

  it('needs two tests in two hours for a best hour', () => {
    const two = byBandSet([resultFixture({ atMs: at(8), allowedBands: '4' }), resultFixture({ atMs: at(8, 30), allowedBands: '4' })], 'cloudflare')
    expect(two[0].bestHour).toBeNull()
  })

  it('names the providers with valid tests, Cloudflare first', () => {
    expect(speedProviders([resultFixture({ provider: 'ookla' }), resultFixture()])).toEqual(['cloudflare', 'ookla'])
    expect(speedProviders([resultFixture({ provider: 'ookla', invalidReason: 'bandChange' }), resultFixture()])).toEqual(['cloudflare'])
  })

  it('sorts by the 5G count, then the median download, and puts missing numbers last', () => {
    const row = (key: string, with5g: number, down: number | null, ping: number | null = null) => ({
      key,
      bands: '4',
      served: null,
      tests: 3,
      with5g,
      medianDownMbps: down,
      medianUpMbps: 1,
      medianBusyPingMs: ping,
      bestHour: null,
      lastMs: 0,
    })
    const rows = [row('a', 0, 200, 300), row('b', 2, 50, null), row('c', 2, 90, 100)]
    expect(sortBandSets(rows, 'with5g', 'desc').map((r) => r.key)).toEqual(['c', 'b', 'a'])
    expect(sortBandSets(rows, 'down', 'desc').map((r) => r.key)).toEqual(['a', 'c', 'b'])
    expect(sortBandSets(rows, 'ping', 'asc').map((r) => r.key)).toEqual(['c', 'a', 'b'])
    expect(sortBandSets(rows, 'ping', 'desc').map((r) => r.key)).toEqual(['a', 'c', 'b'])
  })

  it('rounds an axis up to a whole step from 0', () => {
    expect(niceAxis(132)).toEqual({ max: 150, ticks: [0, 50, 100, 150] })
    expect(niceAxis(7)).toEqual({ max: 8, ticks: [0, 2, 4, 6, 8] })
    expect(niceAxis(0).max).toBeGreaterThan(0)
  })
})

describe('dataUseMb', () => {
  it('counts the tests in a day at the planning figure of their size', () => {
    expect(dataUseMb({ everyH: 3, provider: 'cloudflare', size: 'light' }, false)).toBe(320)
    expect(dataUseMb({ everyH: 24, provider: 'cloudflare', size: 'full' }, false)).toBe(200)
  })
  it('uses the Ookla figure only once its terms are accepted', () => {
    expect(dataUseMb({ everyH: 24, provider: 'ookla', size: 'light' }, true)).toBe(150)
    expect(dataUseMb({ everyH: 24, provider: 'ookla', size: 'light' }, false)).toBe(40)
  })
  it('is 0 for an interval that is still being typed', () => {
    expect(dataUseMb({ everyH: Number.NaN, provider: 'cloudflare', size: 'light' }, false)).toBe(0)
  })
})
