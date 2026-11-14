import type { NetConfig } from './bindings/NetConfig'
import type { Phase } from './bindings/Phase'
import type { SafeApplyView } from './bindings/SafeApplyView'

/**
 * Duplex and quoted MHz label per LTE band. Must agree with Rust `lte_band_meta`
 * (checked by bands.test.ts). B32 is downlink-only ("sdl") and has its own group of chips.
 */
export const LTE_BAND_INFO: Record<number, { duplex: 'fdd' | 'tdd' | 'sdl'; mhz: number }> = {
  1: { duplex: 'fdd', mhz: 2100 },
  2: { duplex: 'fdd', mhz: 1900 },
  3: { duplex: 'fdd', mhz: 1800 },
  4: { duplex: 'fdd', mhz: 1700 },
  5: { duplex: 'fdd', mhz: 850 },
  6: { duplex: 'fdd', mhz: 800 },
  7: { duplex: 'fdd', mhz: 2600 },
  8: { duplex: 'fdd', mhz: 900 },
  9: { duplex: 'fdd', mhz: 1800 },
  10: { duplex: 'fdd', mhz: 1700 },
  11: { duplex: 'fdd', mhz: 1500 },
  12: { duplex: 'fdd', mhz: 700 },
  13: { duplex: 'fdd', mhz: 700 },
  14: { duplex: 'fdd', mhz: 700 },
  17: { duplex: 'fdd', mhz: 700 },
  18: { duplex: 'fdd', mhz: 800 },
  19: { duplex: 'fdd', mhz: 800 },
  20: { duplex: 'fdd', mhz: 800 },
  21: { duplex: 'fdd', mhz: 1500 },
  22: { duplex: 'fdd', mhz: 3500 },
  23: { duplex: 'fdd', mhz: 2000 },
  24: { duplex: 'fdd', mhz: 1500 },
  25: { duplex: 'fdd', mhz: 1900 },
  26: { duplex: 'fdd', mhz: 850 },
  27: { duplex: 'fdd', mhz: 850 },
  28: { duplex: 'fdd', mhz: 700 },
  29: { duplex: 'sdl', mhz: 700 },
  30: { duplex: 'fdd', mhz: 2300 },
  31: { duplex: 'fdd', mhz: 450 },
  32: { duplex: 'sdl', mhz: 1500 },
  33: { duplex: 'tdd', mhz: 1900 },
  34: { duplex: 'tdd', mhz: 2000 },
  35: { duplex: 'tdd', mhz: 1900 },
  36: { duplex: 'tdd', mhz: 1900 },
  37: { duplex: 'tdd', mhz: 1900 },
  38: { duplex: 'tdd', mhz: 2600 },
  39: { duplex: 'tdd', mhz: 1900 },
  40: { duplex: 'tdd', mhz: 2300 },
  41: { duplex: 'tdd', mhz: 2500 },
  42: { duplex: 'tdd', mhz: 3500 },
  43: { duplex: 'tdd', mhz: 3600 },
  44: { duplex: 'tdd', mhz: 700 },
  45: { duplex: 'tdd', mhz: 1400 },
  46: { duplex: 'tdd', mhz: 5200 },
  47: { duplex: 'tdd', mhz: 5900 },
  48: { duplex: 'tdd', mhz: 3500 },
  49: { duplex: 'tdd', mhz: 3500 },
  50: { duplex: 'tdd', mhz: 1500 },
  51: { duplex: 'tdd', mhz: 1400 },
  52: { duplex: 'tdd', mhz: 3300 },
  53: { duplex: 'tdd', mhz: 2500 },
  67: { duplex: 'sdl', mhz: 700 },
  69: { duplex: 'sdl', mhz: 2600 },
  75: { duplex: 'sdl', mhz: 1500 },
  76: { duplex: 'sdl', mhz: 1400 },
}

export type Duplex = 'fdd' | 'tdd' | 'sdl'

/** A band with no uplink, such as B32: it only works as an add-on to another band. */
export const isDownlinkOnly = (band: number): boolean => LTE_BAND_INFO[band]?.duplex === 'sdl'

/** How a band sends and receives. A band missing from the table counts as FDD. */
export const duplexOf = (band: number): Duplex => LTE_BAND_INFO[band]?.duplex ?? 'fdd'

const MAX_BAND = 128

/** Parses a Huawei hex mask; anything that is not 1-32 hex digits counts as empty. */
function parseMask(hex: string): bigint {
  const s = hex.trim()
  return /^[0-9a-fA-F]{1,32}$/.test(s) ? BigInt('0x' + s) : 0n
}

/** Uppercase hex, no prefix, no leading zeros, "0" when empty (same as Rust to_huawei_hex). */
function maskToHex(m: bigint): string {
  return m.toString(16).toUpperCase()
}

/** Band numbers set in the mask, ascending. */
export function supportedBands(supportedHex: string): number[] {
  const m = parseMask(supportedHex)
  const out: number[] = []
  for (let b = 1; b <= MAX_BAND; b++) if (m & (1n << BigInt(b - 1))) out.push(b)
  return out
}

/** Mask of band numbers 1..=128 as Huawei hex (out-of-range numbers are ignored). */
export function bandsToHex(bands: number[]): string {
  let m = 0n
  for (const b of bands) if (Number.isInteger(b) && b >= 1 && b <= MAX_BAND) m |= 1n << BigInt(b - 1)
  return maskToHex(m)
}

/** Splits bands into FDD, TDD and downlink-only (SDL) chips, keeping their order. */
export function groupBands(bands: number[]): Record<Duplex, number[]> {
  const groups: Record<Duplex, number[]> = { fdd: [], tdd: [], sdl: [] }
  for (const b of bands) groups[duplexOf(b)].push(b)
  return groups
}

/** Returns a new config with `band` flipped in `lteBands`; other fields are untouched. */
export function toggleBand(cfg: NetConfig, band: number): NetConfig {
  if (!Number.isInteger(band) || band < 1 || band > MAX_BAND) return cfg
  return { ...cfg, lteBands: maskToHex(parseMask(cfg.lteBands) ^ (1n << BigInt(band - 1))) }
}

export function isEmptyMask(cfg: NetConfig): boolean {
  return parseMask(cfg.lteBands) === 0n
}

export function hasBand(cfg: NetConfig, band: number): boolean {
  return (parseMask(cfg.lteBands) & (1n << BigInt(band - 1))) !== 0n
}

/** Keeps only bands the router supports. */
export function restrictToSupported(hex: string, supportedHex: string): string {
  return maskToHex(parseMask(hex) & parseMask(supportedHex))
}

export function sameConfig(a: NetConfig, b: NetConfig): boolean {
  return (
    parseMask(a.lteBands) === parseMask(b.lteBands) &&
    a.legacyBandRaw === b.legacyBandRaw &&
    a.nrOption === b.nrOption &&
    a.networkMode.kind === b.networkMode.kind &&
    (a.networkMode.kind !== 'raw' || (b.networkMode.kind === 'raw' && a.networkMode.value === b.networkMode.value))
  )
}

/** True when no safe-apply is running, i.e. a new change or a reboot may start (mirrors the backend). */
export function changeClosed(phase: Phase | undefined): boolean {
  return phase?.kind === 'idle' || phase?.kind === 'committed' || phase?.kind === 'reverted'
}

/**
 * True while a band change is open: a safe-apply is running, or a primary-band apply is
 * between its two steps (the phase then reads "committed" but the other bands aren't back
 * yet). Nothing loaded yet counts as not open.
 */
export function changeOpen(view: Pick<SafeApplyView, 'phase' | 'sequence'> | null | undefined): boolean {
  return view != null && (!changeClosed(view.phase) || view.sequence != null)
}

/**
 * True while a band change keeps a speed test from starting: any open change except the trial
 * of its last step. In that trial the new bands are in place, and a test is how the user judges
 * them. Step 1 of a two-step apply is the primary band alone, so its trial doesn't count
 * (mirrors the backend's `change_blocks_test`).
 */
export function changeBlocksTest(view: Pick<SafeApplyView, 'phase' | 'sequence'> | null | undefined): boolean {
  if (!view || !changeOpen(view)) return false
  const lastStep = view.sequence == null || view.sequence.step >= view.sequence.total
  return !(view.phase.kind === 'probation' && lastStep)
}

/** Band numbers in a config, ascending. */
export function bandsOf(cfg: NetConfig | null | undefined): number[] {
  return cfg ? supportedBands(cfg.lteBands) : []
}

/** Returns a config that includes `band` (the same object if it already does). */
export function withBand(cfg: NetConfig, band: number): NetConfig {
  return hasBand(cfg, band) ? cfg : toggleBand(cfg, band)
}

/** How a band in the draft differs from the router's current config. */
export function bandChange(draft: NetConfig, current: NetConfig | null, band: number): 'added' | 'removed' | null {
  if (!current) return null
  const now = hasBand(current, band)
  const next = hasBand(draft, band)
  return next && !now ? 'added' : now && !next ? 'removed' : null
}

/** Minutes and seconds of a duration in whole seconds (negative counts as 0). */
export function splitDuration(totalS: number): { m: number; s: number } {
  const t = Math.max(0, Math.round(totalS))
  return { m: Math.floor(t / 60), s: t % 60 }
}

/** A countdown as "m:ss" in ASCII digits (convert the digits afterwards if needed). */
export function clock(totalS: number): string {
  const { m, s } = splitDuration(totalS)
  return `${m}:${String(s).padStart(2, '0')}`
}

export type TemplatePart = { text: string } | { slot: string }

/**
 * Splits a message such as "Kept {bands}." into text and named slots, so a component can
 * render markup (bold band lists, isolates) in place of a placeholder.
 */
export function templateParts(message: string): TemplatePart[] {
  const out: TemplatePart[] = []
  let last = 0
  for (const m of message.matchAll(/\{(\w+)\}/g)) {
    if (m.index > last) out.push({ text: message.slice(last, m.index) })
    out.push({ slot: m[1] })
    last = m.index + m[0].length
  }
  if (last < message.length) out.push({ text: message.slice(last) })
  return out
}
