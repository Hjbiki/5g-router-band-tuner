import type { BandOutcome } from './bindings/BandOutcome'
import type { ScanResult } from './bindings/ScanResult'
import type { SpeedTestResult } from './bindings/SpeedTestResult'
import { isDownlinkOnly } from './bands'
import type { MessageKey } from './i18n'

/** Below this download rate (Mbps), with no 5G, a band counts as slow. */
const SLOW_MBPS = 10
/** LTE band numbers are bit positions in the mask, and only a band tried alone says anything. */
const loneBand = (hex: string): number | null => {
  const mask = BigInt(`0x${hex}`)
  if (mask === 0n || (mask & (mask - 1n)) !== 0n) return null
  return mask.toString(2).length
}

export type BandTone = 'good' | 'slow' | 'noService'

/** What each past result is called, for the sr-only text and title beside its icon. */
export const TONE_LABEL_KEY: Record<BandTone, MessageKey> = {
  good: 'bands.legend.good',
  slow: 'bands.legend.slow',
  noService: 'bands.legend.noService',
}

/**
 * What the band chips show. For each band the newest evidence decides: a valid speed test on it
 * (5G or a good download is good, otherwise slow) or a lock on that band alone that lost service.
 * A later lock on it alone that was kept takes the "no service" away. Bands with no evidence stay
 * out of the map, and so does a test where the band changed midway. A scan's results count from
 * the moment they exist; a band the router refused was never tried, so it says nothing.
 */
export function bandTones(
  results: SpeedTestResult[],
  outcomes: BandOutcome[],
  scanned: ScanResult[] = [],
): Map<number, BandTone> {
  type Evidence = { at: number; band: number; tone: BandTone | 'kept' }
  const evidence: Evidence[] = outcomes.map((o) => ({
    at: o.atMs,
    band: o.band,
    tone: o.kind === 'noService' ? 'noService' : 'kept',
  }))
  for (const r of results) {
    const band = r.serving.lteBand
    if (band == null || r.invalidReason) continue
    if (r.serving.nrBand != null) evidence.push({ at: r.atMs, band, tone: 'good' })
    else if (r.downloadMbps != null)
      evidence.push({ at: r.atMs, band, tone: r.downloadMbps >= SLOW_MBPS ? 'good' : 'slow' })
  }
  for (const r of scanned) {
    const band = loneBand(r.bands)
    if (band === null) continue
    if (r.outcome === 'noService') evidence.push({ at: r.atMs, band, tone: 'noService' })
    else if (r.outcome === 'measured')
      evidence.push({ at: r.atMs, band, tone: r.got5g || (r.downloadMbps ?? 0) >= SLOW_MBPS ? 'good' : 'slow' })
  }
  evidence.sort((a, b) => b.at - a.at)

  const tones = new Map<number, BandTone>()
  const kept = new Set<number>()
  for (const e of evidence) {
    if (tones.has(e.band)) continue
    if (e.tone === 'kept') kept.add(e.band)
    // A downlink-only band has no service alone wherever you are, so that says nothing about it.
    else if (e.tone === 'noService' && isDownlinkOnly(e.band)) continue
    else if (e.tone !== 'noService' || !kept.has(e.band)) tones.set(e.band, e.tone)
  }
  return tones
}

/** What the last recorded result says about one band, for its chip's popover. */
export type BandRecord = {
  atMs: number
  /** `measured` has speeds. `connected` stayed in service with no speed result. */
  kind: 'measured' | 'connected' | 'noService'
  fromScan: boolean
  downloadMbps: number | null
  uploadMbps: number | null
  pingMs: number | null
  got5g: boolean
  nrBand: number | null
  lteBand: number | null
  ltePci: number | null
  /** The band set allowed at the time, when it was more than this band. */
  allowed: string | null
}

const blank = { downloadMbps: null, uploadMbps: null, pingMs: null, got5g: false, nrBand: null, lteBand: null, ltePci: null, allowed: null }

/**
 * The newest thing recorded about a band: a speed test served on it, a scan of it alone, or a lock
 * on it alone. A lock that was kept says less than the speed test it followed, so it only counts
 * when there's no speed test since the last "no service". `periods` are the times scans ran.
 */
export function lastBandRecord(
  band: number,
  history: SpeedTestResult[],
  outcomes: BandOutcome[],
  scanned: ScanResult[],
  periods: [number, number][],
): BandRecord | null {
  const inScan = (at: number) => periods.some(([from, to]) => at >= from && at <= to)
  const records: BandRecord[] = []
  for (const r of history) {
    if (r.serving.lteBand !== band || r.invalidReason || r.downloadMbps == null) continue
    records.push({
      atMs: r.atMs,
      kind: 'measured',
      fromScan: inScan(r.atMs),
      downloadMbps: r.downloadMbps,
      uploadMbps: r.uploadMbps,
      pingMs: r.idlePingMs,
      got5g: r.serving.nrBand != null,
      nrBand: r.serving.nrBand,
      lteBand: band,
      ltePci: r.serving.ltePci,
      allowed: r.allowedBands && loneBand(r.allowedBands) !== band ? r.allowedBands : null,
    })
  }
  for (const r of scanned) {
    if (loneBand(r.bands) !== band || r.outcome === 'failed') continue
    records.push(
      r.outcome === 'measured'
        ? {
            atMs: r.atMs,
            kind: 'measured',
            fromScan: true,
            downloadMbps: r.downloadMbps,
            uploadMbps: r.uploadMbps,
            pingMs: r.idlePingMs,
            got5g: r.got5g,
            nrBand: r.nrBand,
            lteBand: r.lteBand,
            ltePci: r.ltePci,
            allowed: null,
          }
        : { ...blank, atMs: r.atMs, kind: r.outcome === 'noService' ? 'noService' : 'connected', fromScan: true },
    )
  }
  for (const o of outcomes) {
    if (o.band !== band) continue
    records.push({ ...blank, atMs: o.atMs, kind: o.kind === 'noService' ? 'noService' : 'connected', fromScan: inScan(o.atMs) })
  }
  records.sort((a, b) => b.atMs - a.atMs)

  let connected: BandRecord | null = null
  for (const r of records) {
    if (r.kind === 'measured') return r
    if (r.kind === 'noService') return connected ?? r
    connected ??= r
  }
  return connected
}
