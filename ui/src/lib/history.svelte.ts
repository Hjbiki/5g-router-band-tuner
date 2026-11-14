// The History page's state: the chosen range, and what the backend returned for it.
import type { EventPage } from './bindings/EventPage'
import type { HistoryOverview } from './bindings/HistoryOverview'
import type { HistoryRange } from './bindings/HistoryRange'
import type { SignalSeries } from './bindings/SignalSeries'
import type { SpeedTestResult } from './bindings/SpeedTestResult'
import type { SpeedTestSource } from './bindings/SpeedTestSource'
import type { SignalPoint } from './bindings/SignalPoint'
import type { SpeedTestProvider } from './bindings/SpeedTestProvider'
import { api, onHistoryCleared } from './backend'
import { errorText } from './errors'
import { bandsToHex, supportedBands } from './bands'
import { busyPing } from './speedtest.svelte'
import { TEST_MB } from './scan.svelte'
import type { AutoSpeedTestSettings } from './bindings/AutoSpeedTestSettings'
import { t } from './i18n'

export type RangeKind = '24h' | '7d' | '30d'
/** What the range control shows. A custom range holds the local midnights of its first and last day. */
export type RangeChoice = { kind: RangeKind } | { kind: 'custom'; fromMs: number; toMs: number }

export const RANGE_KINDS: readonly RangeKind[] = ['24h', '7d', '30d']
const RANGE_MS: Record<RangeKind, number> = { '24h': 86_400_000, '7d': 7 * 86_400_000, '30d': 30 * 86_400_000 }

/** Waits this long after the last range change before asking the backend. */
export const LOAD_DEBOUNCE_MS = 200
/** Markers and the band-change count: one page of events is plenty for a chart. */
export const EVENTS_LIMIT = 200
const RANGE_KEY = 'history.range'
/** The range a new user starts on. A range picked before is kept. */
export const DEFAULT_RANGE: RangeKind = '7d'

export const history: {
  range: RangeChoice
  overview: HistoryOverview | null
  signal: SignalSeries | null
  speeds: SpeedTestResult[]
  /** Band changes, router restarts and app starts in the range, newest first. */
  events: EventPage | null
  /** When auto-scans ran, to tell an older test's source. */
  scanPeriods: [number, number][]
  /** The range the loaded data was asked for; null before the first load. */
  loadedRange: HistoryRange | null
  loading: boolean
  error: string | null
} = $state({
  range: readRange(),
  overview: null,
  signal: null,
  speeds: [],
  events: null,
  scanPeriods: [],
  loadedRange: null,
  loading: false,
  error: null,
})

// --- More details sections ----------------------------------------------------------------

export type HistorySection = 'signal' | 'speeds' | 'hours' | 'bandSets' | 'ipRanges' | 'events'
const SECTIONS: readonly HistorySection[] = ['signal', 'speeds', 'hours', 'bandSets', 'ipRanges', 'events']
const OPEN_KEY = 'history.open'

function readOpen(): HistorySection[] {
  try {
    const raw = localStorage.getItem(OPEN_KEY)
    const list: unknown = raw ? JSON.parse(raw) : []
    return Array.isArray(list) ? SECTIONS.filter((s) => list.includes(s)) : []
  } catch {
    return []
  }
}

/** The More details sections open on this PC. They start closed and remember being opened. */
export const sections: { open: HistorySection[] } = $state({ open: readOpen() })

export function sectionOpen(id: HistorySection): boolean {
  return sections.open.includes(id)
}

export function setSectionOpen(id: HistorySection, open: boolean): void {
  if (open === sectionOpen(id)) return
  sections.open = open ? [...sections.open, id] : sections.open.filter((s) => s !== id)
  try {
    localStorage.setItem(OPEN_KEY, JSON.stringify(sections.open))
  } catch {
    // Remembering an open section is a convenience only.
  }
}

/** The local midnight that starts the day `ms` falls on. */
export function startOfDay(ms: number): number {
  const d = new Date(ms)
  return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime()
}

/** The local midnight after the day `ms` falls on; a day isn't always 24 h long. */
function nextDay(ms: number): number {
  const d = new Date(ms)
  return new Date(d.getFullYear(), d.getMonth(), d.getDate() + 1).getTime()
}

/** The query range for a choice. A custom range ends at the end of its last day. */
export function rangeBounds(range: RangeChoice, now: number): HistoryRange {
  const utcOffsetMin = -new Date(now).getTimezoneOffset()
  if (range.kind === 'custom') {
    return { fromMs: startOfDay(range.fromMs), toMs: nextDay(range.toMs) - 1, utcOffsetMin }
  }
  return { fromMs: now - RANGE_MS[range.kind], toMs: now, utcOffsetMin }
}

/** A custom range needs both days, and the first can't come after the last. */
export function validCustom(fromMs: number | null, toMs: number | null): boolean {
  return fromMs !== null && toMs !== null && Number.isFinite(fromMs) && Number.isFinite(toMs) && startOfDay(fromMs) <= startOfDay(toMs)
}

/** `YYYY-MM-DD` for a date input, in local time. */
export function dayValue(ms: number): string {
  const d = new Date(ms)
  const p = (n: number) => String(n).padStart(2, '0')
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`
}

/** A date input's `YYYY-MM-DD` as its local midnight, or null when empty or malformed. */
export function parseDay(value: string): number | null {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value)
  if (!m) return null
  const ms = new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3])).getTime()
  return Number.isFinite(ms) ? ms : null
}

function readRange(): RangeChoice {
  try {
    const raw = localStorage.getItem(RANGE_KEY)
    if (!raw) return { kind: DEFAULT_RANGE }
    const r = JSON.parse(raw) as RangeChoice
    if (r && (RANGE_KINDS as readonly string[]).includes(r.kind)) return { kind: r.kind as RangeKind }
    if (r?.kind === 'custom' && validCustom(r.fromMs, r.toMs)) return { kind: 'custom', fromMs: r.fromMs, toMs: r.toMs }
  } catch {
    // No storage in this window, or something unreadable in it: the default will do.
  }
  return { kind: DEFAULT_RANGE }
}

function saveRange(r: RangeChoice): void {
  try {
    localStorage.setItem(RANGE_KEY, JSON.stringify(r))
  } catch {
    // Remembering the range is a convenience only.
  }
}

let timer: ReturnType<typeof setTimeout> | undefined
let request = 0
let waiters: (() => void)[] = []

/**
 * Reloads everything for the current range, `delayMs` after the last call. Each call supersedes
 * the ones before it, so an answer for an older range is dropped. Resolves when the newest load
 * has finished, whether it worked or not.
 */
export function loadHistory(delayMs: number = LOAD_DEBOUNCE_MS): Promise<void> {
  const id = ++request
  clearTimeout(timer)
  history.loading = true
  return new Promise((resolve) => {
    waiters.push(resolve)
    timer = setTimeout(() => void run(id), delayMs)
  })
}

async function run(id: number): Promise<void> {
  const range = rangeBounds(history.range, Date.now())
  try {
    const overview = await api.historyOverview()
    if (id !== request) return
    let signal: SignalSeries | null = null
    let speeds: SpeedTestResult[] = []
    let events: EventPage | null = null
    let scanPeriods: [number, number][] = []
    if (overview.available) {
      ;[signal, speeds, events, scanPeriods] = await Promise.all([
        api.historySignal(range, 'auto'),
        api.historySpeedtests(range),
        api.historyEvents({
          range,
          limit: EVENTS_LIMIT,
          categories: ['bandChange', 'device', 'app'],
          problemsOnly: false,
          technical: false,
        }),
        // Only older tests need it, so a failure here doesn't hold up the page.
        api.scanPeriods().catch((): [number, number][] => []),
      ])
      if (id !== request) return
    }
    Object.assign(history, { overview, signal, speeds, events, scanPeriods, loadedRange: range, error: null })
  } catch (e) {
    if (id !== request) return
    history.error = errorText(e)
  }
  history.loading = false
  const done = waiters
  waiters = []
  for (const w of done) w()
}

/** Picks a range, remembers it on this PC and reloads. */
export function setRange(r: RangeChoice): Promise<void> {
  history.range = r
  saveRange(r)
  return loadHistory()
}

let following: Promise<void> | null = null

/** Follows `history-cleared`, so a Clear from Settings empties the page. Safe to call repeatedly. */
export function ensureHistory(): Promise<void> {
  following ??= onHistoryCleared(() => void loadHistory(0))
    .then(() => {})
    .catch(() => {
      following = null
    })
  return following
}

/** Test hook: forget the subscription, pending loads and state, as a fresh start would. */
export function resetHistory(): void {
  clearTimeout(timer)
  request++
  waiters = []
  following = null
  sections.open = readOpen()
  Object.assign(history, { range: readRange(), overview: null, signal: null, speeds: [], events: null, scanPeriods: [], loadedRange: null, loading: false, error: null })
}

/**
 * What started a test. Older results don't say: a test inside an auto-scan was the scan's, and
 * any other was the user's.
 */
export function speedSource(r: Pick<SpeedTestResult, 'atMs' | 'source'>, scanPeriods: [number, number][]): SpeedTestSource {
  if (r.source) return r.source
  return scanPeriods.some(([from, to]) => r.atMs >= from && r.atMs <= to) ? 'scan' : 'manual'
}

/** The middle value, or the mean of the middle two; null without any number. */
export function medianOf(values: readonly (number | null | undefined)[]): number | null {
  const v = values.filter((x): x is number => typeof x === 'number' && Number.isFinite(x)).sort((a, b) => a - b)
  if (!v.length) return null
  const mid = v.length >> 1
  return v.length % 2 ? v[mid] : (v[mid - 1] + v[mid]) / 2
}

/** A test that measured one band set from start to end, with a download figure. */
export const validSpeed = (r: SpeedTestResult): boolean => !r.invalidReason && r.downloadMbps != null

/** Below this many seconds without service in a bucket, a dip isn't counted as lost service. */
export const LOST_MIN_S = 30

export type SignalHealth = {
  /** 0 to 100 of the time the router answered, leaving out this app's own changes; null when it never did. */
  inServicePct: number | null
  /** 0 to 100 of the in-service time. */
  on5gPct: number | null
  /** Separate spells without service, not counting minutes of this app's own band changes or scans. */
  lost: number
  /** Router restarts in the page of events. */
  reboots: number
  /** More events than one page holds, so `reboots` may be short. */
  rebootsMore: boolean
}

/**
 * The Signal card's numbers. Minutes in which this app changed bands, scanned, refreshed the IP
 * or switched the APN (`ownChangeS`) never count as lost service: losing it for a moment is part
 * of the change.
 */
export function signalHealth(points: readonly SignalPoint[], events: EventPage | null, bucketMs = 60_000): SignalHealth {
  let observed = 0
  let lostS = 0
  let nrWeight = 0
  let nr = 0
  let lost = 0
  let lastLostAt: number | null = null
  for (const p of points) {
    const own = Math.min(p.ownChangeS, p.observedS)
    const gap = Math.max(0, p.observedS - p.serviceS - own)
    observed += p.observedS - own
    lostS += gap
    if (p.nrShare !== null) {
      nrWeight += p.serviceS
      nr += p.nrShare * p.serviceS
    }
    if (gap >= LOST_MIN_S) {
      // A spell that runs on from the bucket before is the same one.
      if (lastLostAt === null || p.tMs - lastLostAt > bucketMs) lost += 1
      lastLostAt = p.tMs
    }
  }
  const entries = events?.entries ?? []
  return {
    inServicePct: observed > 0 ? Math.max(0, (1 - lostS / observed) * 100) : null,
    on5gPct: nrWeight > 0 ? (nr / nrWeight) * 100 : null,
    lost,
    reboots: entries.filter((e) => e.kind === 'single' && e.item.kind === 'device.reboot').length,
    rebootsMore: events?.nextBeforeMs != null,
  }
}

/** Which empty state the page shows, if any. */
export type HistoryEmpty = 'unavailable' | 'fresh' | 'range' | null

export function emptyState(o: HistoryOverview | null, signal: SignalSeries | null, speeds: SpeedTestResult[]): HistoryEmpty {
  if (!o) return null
  if (!o.available) return 'unavailable'
  if (o.minutes === 0 && o.speedtests === 0) return 'fresh'
  if ((signal?.points.length ?? 0) === 0 && speeds.length === 0) return 'range'
  return null
}

// --- Charts -----------------------------------------------------------------------------------

/** The shape a speed test gets on a chart: the user's own a circle, a scan's a square, an automatic one a triangle. */
export function sourceShape(source: SpeedTestSource): 'circle' | 'square' | 'triangle' {
  return source === 'scan' ? 'square' : source === 'auto' ? 'triangle' : 'circle'
}

/** "B1 + B3" for a list of band numbers. */
export function bandsText(bands: readonly number[]): string {
  return bands.map((b) => `B${b}`).join(' + ')
}

export type MarkerKind = 'applied' | 'kept' | 'reverted' | 'appStart' | 'reboot'
export type SignalMarker = { t: number; kind: MarkerKind; label: string }

/**
 * Vertical lines for the signal chart: each band change where it started (by how it ended),
 * router restarts and app starts. Everything else in the page of events is left out.
 */
export function signalMarkers(page: EventPage | null): SignalMarker[] {
  const out: SignalMarker[] = []
  for (const e of page?.entries ?? []) {
    if (e.kind === 'change') {
      const g = e.group
      const kind: MarkerKind =
        g.outcome.kind === 'kept' ? 'kept' : g.outcome.kind === 'reverted' || g.outcome.kind === 'revertFailed' ? 'reverted' : 'applied'
      const bands = g.to.length ? bandsText(g.to) : t('history.marker.otherBands')
      out.push({ t: g.startedMs, kind, label: t(`history.marker.${kind}`, { bands }) })
    } else if (e.item.kind === 'app.started') {
      out.push({ t: e.item.atMs, kind: 'appStart', label: t('history.marker.appStart') })
    } else if (e.item.kind === 'device.reboot') {
      out.push({ t: e.item.atMs, kind: 'reboot', label: t('history.marker.reboot') })
    }
  }
  return out.sort((a, b) => a.t - b.t)
}

/**
 * Spans of the same allowed band set, for the strip under the signal chart. A bucket covers
 * `bucketMs`; a missing bucket or one without a recorded set ends a span.
 */
export function bandRibbon(points: readonly SignalPoint[], bucketMs: number): { from: number; to: number; label: string }[] {
  const out: { from: number; to: number; label: string; hex: string }[] = []
  for (const p of points) {
    if (!p.allowedBands) continue
    const last = out[out.length - 1]
    if (last && last.hex === p.allowedBands && p.tMs <= last.to) last.to = p.tMs + bucketMs
    else out.push({ from: p.tMs, to: p.tMs + bucketMs, hex: p.allowedBands, label: bandsText(supportedBands(p.allowedBands)) })
  }
  return out.map(({ from, to, label }) => ({ from, to, label }))
}

export type SignalMetric = 'sinr' | 'rsrp' | 'rsrq'
export const SIGNAL_METRICS: readonly SignalMetric[] = ['sinr', 'rsrp', 'rsrq']

/** The y axis for each metric, in dB or dBm. */
export const METRIC_AXIS: Record<SignalMetric, { min: number; max: number; ticks: number[]; unit: 'dB' | 'dBm' }> = {
  sinr: { min: -10, max: 30, ticks: [-10, 0, 10, 20, 30], unit: 'dB' },
  rsrp: { min: -130, max: -60, ticks: [-120, -100, -80, -60], unit: 'dBm' },
  rsrq: { min: -25, max: 0, ticks: [-20, -15, -10, -5, 0], unit: 'dB' },
}

const FIELDS = {
  sinr: ['lteSinr', 'nrSinr'],
  rsrp: ['lteRsrp', 'nrRsrp'],
  rsrq: ['lteRsrq', 'nrRsrq'],
} as const

/** One metric's LTE and 5G values per bucket; `nr` is null when no bucket has a 5G value. */
export function metricValues(
  points: readonly SignalPoint[],
  metric: SignalMetric,
): { lte: { t: number; min: number | null; avg: number | null; max: number | null }[]; nr: { t: number; min: number | null; avg: number | null; max: number | null }[] | null } {
  const [lteKey, nrKey] = FIELDS[metric]
  const lte = points.map((p) => ({ t: p.tMs, ...p[lteKey] }))
  const nr = points.map((p) => ({ t: p.tMs, ...p[nrKey] }))
  return { lte, nr: nr.some((v) => v.avg !== null) ? nr : null }
}

// --- Speed tests ------------------------------------------------------------------------------

export type SpeedMetric = 'down' | 'up' | 'ping'
export const SPEED_METRICS: readonly SpeedMetric[] = ['down', 'up', 'ping']
export const SPEED_SOURCES: readonly SpeedTestSource[] = ['manual', 'scan', 'auto']

/** A test's download, upload or slower ping under load; null when it has none. */
export function speedValue(r: SpeedTestResult, metric: SpeedMetric): number | null {
  const v = metric === 'down' ? r.downloadMbps : metric === 'up' ? r.uploadMbps : busyPing(r).ms
  return typeof v === 'number' && Number.isFinite(v) ? v : null
}

/** A y axis from 0 that fits `top`: a round maximum and 4 or 5 ticks. */
export function niceAxis(top: number): { max: number; ticks: number[] } {
  const want = Number.isFinite(top) && top > 0 ? top : 1
  const raw = want / 4
  const mag = 10 ** Math.floor(Math.log10(raw))
  const step = [1, 2, 2.5, 5, 10].map((m) => m * mag).find((s) => s >= raw) ?? 10 * mag
  const max = Math.ceil(want / step) * step
  const ticks: number[] = []
  for (let v = 0; v <= max + step / 2; v += step) ticks.push(Math.round(v * 1000) / 1000)
  return { max, ticks }
}

/** The `q` (0 to 1) quantile of sorted values, interpolated between neighbours. */
function quantile(sorted: readonly number[], q: number): number | null {
  if (!sorted.length) return null
  const pos = (sorted.length - 1) * q
  const lo = Math.floor(pos)
  const hi = Math.ceil(pos)
  return sorted[lo] + (sorted[hi] - sorted[lo]) * (pos - lo)
}

/** Fewer valid tests than this, and speed by time of day says nothing yet. */
export const MIN_HOUR_TESTS = 6

export type HourBucket = {
  /** The local hour, 0 to 23. */
  hour: number
  n: number
  median: number | null
  p25: number | null
  p75: number | null
  /** One test only: the bar is shown, but hatched. */
  few: boolean
}

/** Valid tests by the local hour they ran in: the median and the middle half of each hour. */
export function byHourOfDay(
  speeds: readonly SpeedTestResult[],
  metric: SpeedMetric = 'down',
): { hours: HourBucket[]; valid: number } {
  const per: number[][] = Array.from({ length: 24 }, () => [])
  let valid = 0
  for (const r of speeds) {
    if (!validSpeed(r)) continue
    const v = speedValue(r, metric)
    if (v === null) continue
    per[new Date(r.atMs).getHours()].push(v)
    valid += 1
  }
  const hours = per.map((list, hour) => {
    const sorted = [...list].sort((a, b) => a - b)
    return {
      hour,
      n: sorted.length,
      median: medianOf(sorted),
      p25: quantile(sorted, 0.25),
      p75: quantile(sorted, 0.75),
      few: sorted.length === 1,
    }
  })
  return { hours, valid }
}

/** The providers that measured at least one valid test, Cloudflare first. */
export function speedProviders(speeds: readonly SpeedTestResult[]): SpeedTestProvider[] {
  const seen = new Set(speeds.filter(validSpeed).map((r) => r.provider))
  return (['cloudflare', 'ookla'] as const).filter((p) => seen.has(p))
}

export type BandSetRow = {
  key: string
  /** The band set as Huawei hex: what "Try these bands" puts in the editor. */
  bands: string
  /** Set for older tests that didn't record the allowed bands: the LTE band that served them. */
  served: number | null
  tests: number
  /** Tests that had 5G. */
  with5g: number
  medianDownMbps: number | null
  medianUpMbps: number | null
  medianBusyPingMs: number | null
  /** The local hour with the fastest median download; null with too few tests to say. */
  bestHour: number | null
  lastMs: number
}

/** Below this many tests, a band set's numbers get a "few tests" note. */
export const FEW_BAND_TESTS = 3

/**
 * Valid tests by one provider, grouped by the band set they ran on. These are the rules of
 * `earlierTests` in scan.svelte.ts: only valid tests with both speeds, and older results without
 * a recorded band set count by the LTE band that served them, as a row of their own.
 */
export function byBandSet(speeds: readonly SpeedTestResult[], provider: SpeedTestProvider): BandSetRow[] {
  const groups = new Map<string, { bands: string; served: number | null; list: SpeedTestResult[] }>()
  for (const r of speeds) {
    if (r.invalidReason || r.provider !== provider) continue
    if (r.downloadMbps == null || r.uploadMbps == null) continue
    let key: string
    let bands: string
    let served: number | null = null
    if (r.allowedBands) {
      bands = bandsToHex(supportedBands(r.allowedBands))
      key = `set:${bands}`
    } else if (r.serving.lteBand !== null) {
      served = r.serving.lteBand
      bands = bandsToHex([served])
      key = `served:${served}`
    } else continue
    const g = groups.get(key) ?? { bands, served, list: [] }
    g.list.push(r)
    groups.set(key, g)
  }
  return [...groups.entries()].map(([key, g]) => ({
    key,
    bands: g.bands,
    served: g.served,
    tests: g.list.length,
    with5g: g.list.filter((r) => r.serving.nrBand != null).length,
    medianDownMbps: medianOf(g.list.map((r) => r.downloadMbps)),
    medianUpMbps: medianOf(g.list.map((r) => r.uploadMbps)),
    medianBusyPingMs: medianOf(g.list.map((r) => busyPing(r).ms)),
    bestHour: bestHour(g.list),
    lastMs: Math.max(...g.list.map((r) => r.atMs)),
  }))
}

/** How much faster one median download must be than the next before the page calls it faster. */
export const CLEAR_LEAD = 0.2
/** Below this many tests, an IP range isn't compared with the others. */
export const MIN_RANGE_TESTS = 2

export type IpRangeRow = {
  /** "a.b", the first two numbers of the internet address; null for tests that didn't record it. */
  range: string | null
  tests: number
  medianDownMbps: number | null
  medianUpMbps: number | null
  medianBusyPingMs: number | null
  lastMs: number
  /** Enough tests to set beside the others. */
  compared: boolean
}

/** Whether any test in the list recorded its IP range. */
export function hasIpRanges(speeds: readonly SpeedTestResult[]): boolean {
  return speeds.some((r) => !!r.ipRange)
}

/**
 * Valid tests by one provider, grouped by the IP range they started on, with the rules of
 * `byBandSet`. Compared ranges come first, fastest median download first; ranges with too few
 * tests follow, and tests without a range come last.
 */
export function byIpRange(speeds: readonly SpeedTestResult[], provider: SpeedTestProvider): IpRangeRow[] {
  const groups = new Map<string | null, SpeedTestResult[]>()
  for (const r of speeds) {
    if (r.invalidReason || r.provider !== provider) continue
    if (r.downloadMbps == null || r.uploadMbps == null) continue
    const key = r.ipRange || null
    groups.set(key, [...(groups.get(key) ?? []), r])
  }
  const rows = [...groups.entries()].map(([range, list]) => ({
    range,
    tests: list.length,
    medianDownMbps: medianOf(list.map((r) => r.downloadMbps)),
    medianUpMbps: medianOf(list.map((r) => r.uploadMbps)),
    medianBusyPingMs: medianOf(list.map((r) => busyPing(r).ms)),
    lastMs: Math.max(...list.map((r) => r.atMs)),
    compared: range !== null && list.length >= MIN_RANGE_TESTS,
  }))
  const rank = (r: IpRangeRow) => (r.range === null ? 2 : r.compared ? 0 : 1)
  return rows.sort((a, b) => rank(a) - rank(b) || (b.medianDownMbps ?? -1) - (a.medianDownMbps ?? -1) || b.tests - a.tests)
}

/** The range shown as faster while the section is closed: compared, and clearly ahead of the next. */
export function fasterRange(rows: readonly IpRangeRow[]): string | null {
  const compared = rows.filter((r) => r.compared && r.medianDownMbps !== null)
  if (compared.length < 2) return null
  const [a, b] = compared
  return (a.medianDownMbps as number) >= (b.medianDownMbps as number) * (1 + CLEAR_LEAD) ? a.range : null
}

/** The hour with the fastest median download. It needs 2 tests, in 2 different hours. */
function bestHour(list: readonly SpeedTestResult[]): number | null {
  if (list.length < 2) return null
  const hours = byHourOfDay(list).hours.filter((h) => h.median !== null)
  if (hours.length < 2) return null
  return hours.reduce((a, b) => ((b.median as number) > (a.median as number) ? b : a)).hour
}

export type BandSortKey = 'with5g' | 'tests' | 'down' | 'up' | 'ping' | 'last'

const SORT_VALUE: Record<BandSortKey, (r: BandSetRow) => number | null> = {
  with5g: (r) => r.with5g,
  tests: (r) => r.tests,
  down: (r) => r.medianDownMbps,
  up: (r) => r.medianUpMbps,
  ping: (r) => r.medianBusyPingMs,
  last: (r) => r.lastMs,
}

/**
 * Sorted for the table. The default (5G, descending) is by the number of tests with 5G, then the
 * median download; every other column breaks ties on the median download too. Missing numbers go
 * last either way. There's no score: each column means what it says.
 */
export function sortBandSets(rows: readonly BandSetRow[], key: BandSortKey, dir: 'asc' | 'desc'): BandSetRow[] {
  const val = SORT_VALUE[key]
  const sign = dir === 'asc' ? 1 : -1
  return [...rows].sort((a, b) => {
    const va = val(a)
    const vb = val(b)
    if (va !== vb) {
      if (va === null) return 1
      if (vb === null) return -1
      return sign * (va - vb)
    }
    return (b.medianDownMbps ?? -1) - (a.medianDownMbps ?? -1)
  })
}

/** Above this many MB a day, the automatic-test settings show a warning. */
export const AUTO_TEST_WARN_MB = 300

/**
 * About how much data automatic tests use in a day: a test every `everyH` hours, at the planning
 * figure of its size. An Ookla test ignores the size (and needs its terms accepted).
 */
export function dataUseMb(cfg: Pick<AutoSpeedTestSettings, 'everyH' | 'provider' | 'size'>, ooklaTermsAccepted: boolean): number {
  if (!Number.isFinite(cfg.everyH) || cfg.everyH <= 0) return 0
  const kind = cfg.provider === 'ookla' && ooklaTermsAccepted ? 'ookla' : cfg.size
  return (24 / cfg.everyH) * TEST_MB[kind]
}
