import type { Quality } from './bindings/Quality'
import type { LteSignal } from './bindings/LteSignal'
import type { NrSignal } from './bindings/NrSignal'
import type { Rat } from './bindings/Rat'
import type { Signal } from './bindings/Signal'
import { t, type MessageKey } from './i18n'

export type DigitStyle = 'western' | 'arabicIndic'
const DASH = '—'

function toArabicIndic(s: string): string {
  return s.replace(/[0-9]/g, (d) => String.fromCharCode(0x0660 + Number(d)))
}

export function formatNumber(
  n: number | null | undefined,
  opts: { digits?: DigitStyle; fraction?: number } = {},
): string {
  if (n === null || n === undefined || !Number.isFinite(n)) return DASH
  const f = opts.fraction
  const s = new Intl.NumberFormat('en-US', {
    useGrouping: false,
    minimumFractionDigits: f ?? 0,
    maximumFractionDigits: f ?? 2,
  }).format(n)
  // Keep the ASCII hyphen-minus whatever Intl emits.
  const ascii = s.replace(/−/g, '-')
  return opts.digits === 'arabicIndic' ? toArabicIndic(ascii) : ascii
}

/** One fraction digit when the value is not whole; whole numbers print without decimals. */
export function formatMeasure(
  n: number | null | undefined,
  unit: 'dBm' | 'dB' | 'MHz',
  digits: DigitStyle = 'western',
): string {
  if (n === null || n === undefined || !Number.isFinite(n)) return DASH
  const rounded = Math.round(n * 10) / 10
  const fraction = Number.isInteger(rounded) ? 0 : 1
  return `${formatNumber(n, { digits, fraction })} ${unit}`
}

// Inclusive lower bounds for excellent/good/fair; mirrors router-core bands/quality.rs.
export const THRESHOLDS = {
  rsrp: [-80, -90, -100],
  rsrq: [-10, -15, -20],
  sinr: [20, 13, 0],
} as const

export function qualityOf(metric: 'rsrp' | 'rsrq' | 'sinr', v: number | null): Quality | null {
  if (v === null || v === undefined || Number.isNaN(v)) return null
  const [excellent, good, fair] = THRESHOLDS[metric]
  if (v >= excellent) return 'excellent'
  if (v >= good) return 'good'
  if (v >= fair) return 'fair'
  return 'poor'
}

export function bandLabel(lte: LteSignal | null, nr: NrSignal | null): string {
  const parts: string[] = []
  if (lte?.band != null) parts.push(`B${lte.band}`)
  if (nr?.band != null) parts.push(`n${nr.band.primary}`)
  return parts.length ? parts.join(' + ') : DASH
}

const NET_TYPE_KEYS = {
  1: 'netType.gsm', 2: 'netType.gprs', 3: 'netType.edge',
  41: 'netType.umts', 42: 'netType.hsdpa', 43: 'netType.hsupa', 44: 'netType.hspa',
  45: 'netType.hspaPlus', 46: 'netType.dcHspaPlus',
  101: 'netType.lte', 1011: 'netType.lteCa', 111: 'netType.nr',
} as const satisfies Record<number, MessageKey>

const SERVING_KEYS = { gsm: 'netType.gsm', umts: 'netType.umts', lte: 'netType.lte', nr: 'netType.nr' } as const

/**
 * Router CurrentNetworkTypeEx code to a label; unknown codes fall back to the serving RAT.
 * The H122 reports plain LTE (101) during carrier aggregation and 5G NSA, so the live signal
 * upgrades an LTE label: a secondary cell means CA, an NR leg means "+ 5G NSA".
 */
export function networkTypeLabel(code: number | null | undefined, signal: Signal | null | undefined): string {
  if (code === 0) return DASH
  const serving = signal?.serving
  let key: MessageKey | undefined =
    (code != null ? (NET_TYPE_KEYS as Record<number, MessageKey>)[code] : undefined) ??
    (serving ? SERVING_KEYS[serving] : undefined)
  if (!key) return DASH
  if (key !== 'netType.lte' && key !== 'netType.lteCa') return t(key)
  if (key === 'netType.lte' && signal?.lte?.sccPci != null) key = 'netType.lteCa'
  return signal?.nr ? t('netType.withNsa', { lte: t(key) }) : t(key)
}

/** Wraps text in a left-to-right isolate so numbers and IDs keep their order inside RTL text. */
export function isolate(s: string | number): string {
  return `⁦${s}⁩`
}

/** Wraps text in a first-strong isolate: a name keeps its own direction inside a sentence. */
export function isolateText(s: string): string {
  return '\u2068' + s + '\u2069'
}

/** A measurement without its unit: one fraction digit when not whole, like `formatMeasure`. */
export function formatFigure(n: number | null | undefined, digits: DigitStyle = 'western'): string {
  if (n === null || n === undefined || !Number.isFinite(n)) return DASH
  const rounded = Math.round(n * 10) / 10
  return formatNumber(n, { digits, fraction: Number.isInteger(rounded) ? 0 : 1 })
}

/** Filled steps of the 4-step quality meter: poor 1, fair 2, good 3, excellent 4; none 0. */
export function qualitySteps(q: Quality | null): 0 | 1 | 2 | 3 | 4 {
  return q === 'excellent' ? 4 : q === 'good' ? 3 : q === 'fair' ? 2 : q === 'poor' ? 1 : 0
}

/** Duplex scheme as its technical abbreviation; the same in every language. */
export function duplexLabel(d: 'fdd' | 'tdd' | 'sdl' | 'sdlOnly' | null | undefined): string {
  if (!d) return DASH
  return d === 'fdd' ? 'FDD' : d === 'tdd' ? 'TDD' : 'SDL'
}

/** "Updated N s ago" for the Signal page: seconds under a minute, then whole minutes. */
export function updatedAgo(
  capturedAtMs: number | null | undefined,
  nowMs: number,
  digits: DigitStyle = 'western',
): string {
  if (capturedAtMs == null) return ''
  const s = Math.max(0, Math.floor((nowMs - capturedAtMs) / 1000))
  if (s < 1) return t('dashboard.updatedNow')
  if (s < 60) return t('dashboard.updatedSeconds', { n: formatNumber(s, { digits }) })
  return t('dashboard.updatedMinutes', { n: formatNumber(Math.floor(s / 60), { digits }) })
}
