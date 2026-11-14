import type { Live } from './bindings/Live'
import type { ServingCell } from './bindings/ServingCell'
import type { SpeedPhase } from './bindings/SpeedPhase'
import type { SpeedTestEvent } from './bindings/SpeedTestEvent'
import type { SpeedTestProvider } from './bindings/SpeedTestProvider'
import type { SpeedTestResult } from './bindings/SpeedTestResult'
import type { SpeedTestSettings } from './bindings/SpeedTestSettings'
import { api, onSpeedTest } from './backend'
import { errorText } from './errors'
import { formatNumber, type DigitStyle } from './format'
import { t, currentLanguage, type MessageKey } from './i18n'
import { updateSettings } from './store.svelte'
import { rangeText } from './network'

/** Replies slower than this make calls stutter; faster than `LAT_LOW` feel instant. */
export const LAT_HIGH = 300
export const LAT_LOW = 100
/** Below this upload rate (Mbps), upload limits video calls and sending files. */
export const UP_SLOW = 5
export const DOWN_SLOW = 10

export const HISTORY_SHOWN = 200

export type Stage = 'idlePing' | 'download' | 'upload' | 'busy'
export const STAGES: Stage[] = ['idlePing', 'download', 'upload', 'busy']

export const speed: {
  running: boolean
  phase: SpeedPhase | null
  fraction: number
  bytesUsed: number
  live: Live
  start: { provider: SpeedTestProvider; serving: ServingCell } | null
  history: SpeedTestResult[]
  /** Text for the last failure or refusal; null when there is none. */
  error: string | null
  /** The technical cause of the last failure, shown under the plain text. */
  errorCause: string | null
  /** When `error` was set, so a card can tell it from an older or newer result. */
  errorAtMs: number | null
  cancelled: boolean
} = $state({
  running: false,
  phase: null,
  fraction: 0,
  bytesUsed: 0,
  live: emptyLive(),
  start: null,
  history: [],
  error: null,
  errorCause: null,
  errorAtMs: null,
  cancelled: false,
})

function emptyLive(): Live {
  return { idlePingMs: null, downloadMbps: null, uploadMbps: null, pingDownMs: null, pingUpMs: null }
}

/** Backend refusal strings mapped to dictionary keys. */
const REFUSALS: Record<string, MessageKey> = {
  'Finish or revert the current band change first.': 'speedtest.error.busy',
  'a speed test is already running': 'speedtest.error.running',
  'an auto-scan is running': 'scan.blockedRunning',
}

export function refusalText(e: unknown): string {
  const key = typeof e === 'string' ? REFUSALS[e] : undefined
  return key ? t(key) : errorText(e)
}

export function ingestSpeedEvent(e: SpeedTestEvent): void {
  switch (e.kind) {
    case 'started':
      speed.running = true
      speed.phase = null
      speed.fraction = 0
      speed.bytesUsed = 0
      speed.live = emptyLive()
      speed.start = { provider: e.provider, serving: e.serving }
      speed.error = null
      speed.errorAtMs = null
      speed.cancelled = false
      break
    case 'progress':
      speed.running = true
      speed.phase = e.phase
      speed.fraction = e.fraction
      speed.bytesUsed = e.bytesUsed
      // The Ookla CLI reports nothing until the end; keep what was shown.
      if (e.phase !== 'installing' && e.phase !== 'external' && !(e.phase === 'done' && isEmpty(e.live))) {
        speed.live = e.live
      }
      break
    case 'finished': {
      const { kind: _kind, ...result } = e
      speed.running = false
      speed.phase = null
      // A success always replaces an earlier failure.
      speed.error = null
      speed.errorCause = null
      speed.errorAtMs = null
      speed.history.unshift(result)
      if (speed.history.length > HISTORY_SHOWN) speed.history.length = HISTORY_SHOWN
      break
    }
    case 'failed':
      speed.running = false
      speed.phase = null
      // A plain reason first; the cause (from the server or the Ookla CLI) underneath.
      speed.error = t(`speedtest.fail.${e.reason ?? 'other'}`)
      speed.errorCause = e.message
      speed.errorAtMs = Date.now()
      break
    case 'cancelled':
      speed.running = false
      speed.phase = null
      speed.cancelled = true
      break
  }
}

function isEmpty(l: Live): boolean {
  return Object.values(l).every((v) => v === null)
}

let started: Promise<void> | null = null

/** Loads the history and follows `speedtest` events. Safe to call from every component. */
export function ensureSpeedTest(): Promise<void> {
  started ??= (async () => {
    await onSpeedTest(ingestSpeedEvent)
    const [history, running] = await Promise.all([api.speedTestHistory(), api.speedTestRunning()])
    // Results that arrived while loading are already at the front.
    const seen = new Set(speed.history.map((r) => r.atMs))
    speed.history.push(...history.filter((r) => !seen.has(r.atMs)))
    if (running) speed.running = true
  })().catch((e) => {
    started = null
    speed.error = errorText(e)
  })
  return started
}

/** Test hook: forget the subscription and state. */
export function resetSpeedTest(): void {
  started = null
  Object.assign(speed, {
    running: false,
    phase: null,
    fraction: 0,
    bytesUsed: 0,
    live: emptyLive(),
    start: null,
    history: [],
    error: null,
    errorCause: null,
    errorAtMs: null,
    cancelled: false,
  })
}

export async function startTest(): Promise<void> {
  speed.error = null
  speed.errorCause = null
  speed.errorAtMs = null
  speed.cancelled = false
  try {
    await api.runSpeedTest()
  } catch (e) {
    speed.error = refusalText(e)
    speed.errorAtMs = Date.now()
  }
}

export async function cancelTest(): Promise<void> {
  try {
    await api.cancelSpeedTest()
  } catch (e) {
    speed.error = errorText(e)
  }
}

/**
 * Saves the speed-test settings through the shared settings queue, so the patch lands on the
 * newest settings and can't overwrite a save made elsewhere at the same time.
 */
export async function saveSpeedSettings(patch: Partial<SpeedTestSettings>): Promise<void> {
  await updateSettings((s) => ({ ...s, speedTest: { ...s.speedTest, ...patch } }))
}

/**
 * Why a result can't be compared with the others (the band or cell changed during the test),
 * or null. The backend marks such results; older results and builds have no marker.
 */
export function invalidReason(r: SpeedTestResult): string | null {
  return r.invalidReason ?? null
}

/** Why a result isn't comparable, in words: the internet address or the band changed mid-test. */
export function notComparableKey(r: SpeedTestResult): MessageKey {
  return r.invalidReason === 'ipChanged' ? 'speedtest.history.notComparableIp' : 'speedtest.history.notComparable'
}

/** The test's internet address range, "10.45.x.x", or null for older results. */
export function ipRangeLabel(r: SpeedTestResult): string | null {
  return r.ipRange ? rangeText(r.ipRange) : null
}

/** One 0..1 figure for the whole test, or null while it can't be known (installing, Ookla's CLI, not started). */
export function overallFraction(phase: SpeedPhase | null, fraction: number): number | null {
  const f = Math.min(1, Math.max(0, Number.isFinite(fraction) ? fraction : 0))
  switch (phase) {
    case 'idlePing':
      return 0.1 * f
    case 'download':
      return 0.1 + 0.45 * f
    case 'upload':
      return 0.55 + 0.45 * f
    case 'done':
      return 1
    default:
      return null
  }
}

/** The words for what a running test is doing now. */
export function stageKey(phase: SpeedPhase | null): MessageKey {
  switch (phase) {
    case null:
      return 'speedtest.home.stage.starting'
    case 'idlePing':
    case 'download':
    case 'upload':
      return `speedtest.home.stage.${phase}`
    case 'installing':
      return 'speedtest.installing'
    case 'external':
      return 'speedtest.external'
    case 'done':
      return 'speedtest.home.stage.saving'
  }
}

// --- Formatting -------------------------------------------------------------------------------

export function busyPing(r: { pingDownMs: number | null; pingUpMs: number | null }): {
  ms: number | null
  during: 'download' | 'upload' | null
} {
  const d = r.pingDownMs
  const u = r.pingUpMs
  if (d === null && u === null) return { ms: null, during: null }
  if (u !== null && (d === null || u >= d)) return { ms: u, during: 'upload' }
  return { ms: d, during: 'download' }
}

export type LatLevel = 'low' | 'moderate' | 'high'

export function latLevel(ms: number | null): LatLevel | null {
  if (ms === null) return null
  if (ms > LAT_HIGH) return 'high'
  if (ms >= LAT_LOW) return 'moderate'
  return 'low'
}

/** Mbps with one decimal below 100, whole numbers above. */
export function mbps(n: number | null, digits: DigitStyle = 'western'): string {
  if (n === null) return formatNumber(null)
  return formatNumber(n, { digits, fraction: n < 100 ? 1 : 0 })
}

export function ms(n: number | null, digits: DigitStyle = 'western'): string {
  return formatNumber(n === null ? null : Math.round(n), { digits })
}

/** "1.4 s" from 1000 ms up, else "610 ms". */
export function duration(n: number, digits: DigitStyle = 'western'): string {
  return n >= 1000
    ? t('speedtest.unit.seconds', { n: formatNumber(n / 1000, { digits, fraction: 1 }) })
    : t('speedtest.unit.millis', { n: ms(n, digits) })
}

/** "B3 + n41" (bands only); "—" when nothing was serving. */
export function servingBands(s: ServingCell): string {
  const parts: string[] = []
  if (s.lteBand !== null) parts.push(`B${s.lteBand}`)
  if (s.nrBand !== null) parts.push(`n${s.nrBand}`)
  return parts.length ? parts.join(' + ') : '—'
}

/** "PCI 103", with "no 5G" when LTE served alone. */
export function servingDetail(s: ServingCell): string {
  const bits: string[] = []
  if (s.ltePci !== null) bits.push(t('speedtest.history.pci', { pci: s.ltePci }))
  if (s.lteBand !== null && s.nrBand === null) bits.push(t('speedtest.history.noNr'))
  return bits.join(' · ')
}

export function whenLabel(atMs: number, now: number = Date.now(), digits: DigitStyle = 'western'): string {
  const d = new Date(atMs)
  const lang = currentLanguage()
  const numberingSystem = digits === 'arabicIndic' ? 'arab' : 'latn'
  const time = new Intl.DateTimeFormat(lang, { hour: '2-digit', minute: '2-digit', hourCycle: 'h23', numberingSystem }).format(d)
  const day = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime()
  const diff = Math.round((day(new Date(now)) - day(d)) / 86_400_000)
  if (diff === 0) return t('speedtest.history.today', { time })
  if (diff === 1) return t('speedtest.history.yesterday', { time })
  const date = new Intl.DateTimeFormat(lang, { day: 'numeric', month: 'short', numberingSystem }).format(d)
  return t('speedtest.history.dated', { date, time })
}

export type Diagnosis = { headline: string; detail: string }

/** One sentence about what limits this connection, worst problem first. */
export function diagnose(r: SpeedTestResult | null, digits: DigitStyle = 'western'): Diagnosis {
  if (!r) return { headline: t('speedtest.diag.none'), detail: t('speedtest.diag.noneDetail') }
  const busy = busyPing(r)
  const up = r.uploadMbps
  const down = r.downloadMbps
  if (down === null || up === null) {
    return { headline: t('speedtest.diag.partial'), detail: t('speedtest.diag.partialDetail') }
  }
  const lagging = busy.ms !== null && busy.ms > LAT_HIGH
  if (up < UP_SLOW && (busy.during === 'upload' || !lagging)) {
    return {
      headline: t('speedtest.diag.upload'),
      detail: lagging
        ? t('speedtest.diag.uploadLag', { time: duration(busy.ms!, digits) })
        : t('speedtest.diag.uploadSlow', { speed: t('speedtest.unit.speed', { n: mbps(up, digits) }) }),
    }
  }
  if (lagging) {
    return {
      headline: t('speedtest.diag.lag'),
      detail: t(busy.during === 'upload' ? 'speedtest.diag.lagUp' : 'speedtest.diag.lagDown', {
        time: duration(busy.ms!, digits),
      }),
    }
  }
  if (down < DOWN_SLOW) {
    return {
      headline: t('speedtest.diag.download'),
      detail: t('speedtest.diag.downloadDetail', { speed: t('speedtest.unit.speed', { n: mbps(down, digits) }) }),
    }
  }
  return {
    headline: t('speedtest.diag.good'),
    detail: busy.ms !== null
      ? t('speedtest.diag.goodDetail', { time: duration(busy.ms, digits) })
      : t('speedtest.diag.goodPlain'),
  }
}

export function fallbackNote(r: SpeedTestResult): string | null {
  if (!r.fallback) return null
  return t('speedtest.fallback.note', { reason: t(`speedtest.fallback.${r.fallback}`) })
}
