import type { ScanFile } from './bindings/ScanFile'
import type { ScanMode } from './bindings/ScanMode'
import type { ScanResult } from './bindings/ScanResult'
import type { SpeedTestProvider } from './bindings/SpeedTestProvider'
import type { SpeedTestResult } from './bindings/SpeedTestResult'
import type { SpeedTestSettings } from './bindings/SpeedTestSettings'
import { api, onScan } from './backend'
import { bandsToHex, isDownlinkOnly, supportedBands } from './bands'
import type { BandTone } from './bandHistory'

export const scan: { file: ScanFile | null } = $state({ file: null })

/** The scan owns the router's bands right now. */
export function scanActive(): boolean {
  const s = scan.file?.status
  return s === 'running' || s === 'returning'
}

let started: Promise<void> | null = null

/** Loads the last scan and follows the running one. Safe to call often. */
export function ensureScan(): Promise<void> {
  started ??= (async () => {
    await onScan((f) => (scan.file = f))
    scan.file = (await api.getScan()) ?? null
  })().catch(() => {
    started = null
  })
  return started
}

/** For tests. */
export function resetScan(): void {
  started = null
  scan.file = null
}

export type ScanPriority = 'balanced' | 'download' | 'upload' | 'ping'
export const SCAN_PRIORITIES: readonly ScanPriority[] = ['balanced', 'download', 'upload', 'ping']

/** The slower of the two pings under load: what a video call feels. */
export function busyPingOf(r: ScanResult): number | null {
  const values = [r.pingDownMs, r.pingUpMs].filter((v): v is number => v !== null)
  return values.length ? Math.max(...values) : null
}

const measured = (r: ScanResult) => r.outcome === 'measured' && r.downloadMbps !== null && r.uploadMbps !== null

/** Speed tests a scan can run on each config: more steady the result, at more time and data. */
export const SCAN_REPEATS = [1, 2, 3] as const

/** One test can't rank bands: the same band set can measure 30 and 120 Mbps minutes apart. */
export const defaultRepeats = (mode: ScanMode): number => (mode === 'thorough' ? 3 : 2)

/** The longest a scan waits for a config to reconnect; a manual change waits the full setting. */
export const SCAN_REATTACH_S = 60

/** How far back an earlier speed test still says something about a band set. */
const EARLIER_WINDOW_MS = 7 * 24 * 60 * 60 * 1000

/** A band set the recommendation can weigh: this scan's numbers and recent earlier tests. */
export type ScanCandidate = {
  bands: string
  downloadMbps: number
  uploadMbps: number
  busyPingMs: number | null
  got5g: boolean
  nrBand: number | null
  /** This scan measured it. Otherwise all the numbers come from earlier tests. */
  measuredNow: boolean
  /** Earlier speed tests in these numbers. */
  earlier: number
  /** All the speed tests in these numbers, this scan's included. */
  tests: number
  /** The lowest and highest download behind the average. */
  lowMbps: number
  highMbps: number
}

/** A swing this big between tests of one band set is worth saying out loud. */
const WIDE_SWING = 1.5
export const swingsWidely = (c: ScanCandidate): boolean => c.lowMbps > 0 && c.highMbps / c.lowMbps >= WIDE_SWING

/**
 * Tests from before this scan on the same band set, by the same provider, since two providers
 * don't measure alike. Older results don't say which bands were allowed, so for a band tried alone
 * they count when they were served on it. A band set of several needs the recorded set.
 */
export function earlierTests(
  bands: string,
  history: SpeedTestResult[],
  provider: SpeedTestProvider,
  before: number,
  now: number,
): SpeedTestResult[] {
  const alone = supportedBands(bands)
  return history.filter((r) => {
    if (r.atMs >= before || now - r.atMs > EARLIER_WINDOW_MS) return false
    if (r.invalidReason || r.provider !== provider) return false
    if (r.downloadMbps == null || r.uploadMbps == null) return false
    if (r.allowedBands) return BigInt(`0x${r.allowedBands}`) === BigInt(`0x${bands}`)
    return alone.length === 1 && r.serving.lteBand === alone[0]
  })
}

const average = (values: number[]) => values.reduce((a, b) => a + b, 0) / values.length
const slowerPing = (r: { pingDownMs?: number | null; pingUpMs?: number | null }) =>
  busyPingOf({ pingDownMs: r.pingDownMs ?? null, pingUpMs: r.pingUpMs ?? null } as ScanResult)

function candidate(bands: string, now: ScanResult | null, earlier: SpeedTestResult[]): ScanCandidate | null {
  const measuredNow = now !== null && measured(now)
  if (!measuredNow && earlier.length === 0) return null
  // A scan's own average counts once per test behind it.
  const own = measuredNow ? Array.from({ length: Math.max(now.tests, 1) }, () => now) : []
  const pings = [...own.map(slowerPing), ...earlier.map(slowerPing)].filter((v): v is number => v !== null)
  const downs = [
    ...(measuredNow ? [now.downloadMinMbps ?? now.downloadMbps, now.downloadMaxMbps ?? now.downloadMbps] : []),
    ...earlier.map((r) => r.downloadMbps),
  ].filter((v): v is number => v != null)
  const latest = earlier.reduce<SpeedTestResult | null>((a, b) => (a && a.atMs > b.atMs ? a : b), null)
  return {
    bands,
    downloadMbps: average([...own.map((r) => r.downloadMbps as number), ...earlier.map((r) => r.downloadMbps as number)]),
    uploadMbps: average([...own.map((r) => r.uploadMbps as number), ...earlier.map((r) => r.uploadMbps as number)]),
    busyPingMs: pings.length ? average(pings) : null,
    got5g: measuredNow ? now.got5g : latest?.serving.nrBand != null,
    nrBand: measuredNow ? now.nrBand : (latest?.serving.nrBand ?? null),
    measuredNow,
    earlier: earlier.length,
    tests: own.length + earlier.length,
    lowMbps: Math.min(...downs),
    highMbps: Math.max(...downs),
  }
}

/** One band set in the results. `numbers` is what it's ranked on; null when it has none to rank. */
export type ScanRow = { bands: string; result: ScanResult | null; numbers: ScanCandidate | null }

/** The one ranking: the table lists `rows` in this order, and `pick` is always the first of them. */
export type ScanRanking = { rows: ScanRow[]; pick: ScanCandidate | null }

/**
 * Best first, for the chosen priority. Only valid numbers count: this scan's, plus recent earlier
 * tests of the same band set. Balanced puts band sets with 5G first, then weighs download 40%,
 * upload 30% and ping under load 30%, each against the best seen, because a fast download with a
 * stalled upload still makes calls unusable. A band set this scan found dead is out, whatever it
 * did before. Band sets with no numbers come last, in the order they ran.
 *
 * There's no pick when a band set with no numbers could still beat the first, which is what the
 * lack of a speed result must never decide: the first has no 5G, or nothing else has numbers.
 */
export function rankScan(
  file: ScanFile,
  history: SpeedTestResult[],
  priority: ScanPriority,
  provider: SpeedTestProvider,
  now: number,
): ScanRanking {
  const asked = new Set<string>([...file.queue, ...file.results.map((r) => r.bands)])
  const ranked: (ScanRow & { numbers: ScanCandidate })[] = []
  const rest: ScanRow[] = []
  let unknown = 0
  for (const bands of asked) {
    const result = file.results.find((r) => r.bands === bands) ?? null
    const dead = result?.outcome === 'noService'
    const numbers = dead ? null : candidate(bands, result, earlierTests(bands, history, provider, file.startedMs, now))
    if (numbers) ranked.push({ bands, result, numbers })
    else {
      if (!dead) unknown += 1
      // A band set the scan hasn't reached, with no earlier tests, has nothing to show.
      if (result) rest.push({ bands, result, numbers: null })
    }
  }
  const all = ranked.map((r) => r.numbers)
  const maxDown = Math.max(0, ...all.map((c) => c.downloadMbps))
  const maxUp = Math.max(0, ...all.map((c) => c.uploadMbps))
  const busy = all.map((c) => c.busyPingMs).filter((v): v is number => v !== null && v > 0)
  const minBusy = busy.length ? Math.min(...busy) : 0
  const score = (c: ScanCandidate) =>
    0.4 * (maxDown > 0 ? c.downloadMbps / maxDown : 0) +
    0.3 * (maxUp > 0 ? c.uploadMbps / maxUp : 0) +
    0.3 * (c.busyPingMs && c.busyPingMs > 0 ? minBusy / c.busyPingMs : 0)
  const ping = (c: ScanCandidate) => c.busyPingMs ?? Number.POSITIVE_INFINITY
  const order: Record<ScanPriority, (a: ScanCandidate, b: ScanCandidate) => number> = {
    balanced: (a, b) => Number(b.got5g) - Number(a.got5g) || score(b) - score(a),
    download: (a, b) => b.downloadMbps - a.downloadMbps,
    upload: (a, b) => b.uploadMbps - a.uploadMbps,
    ping: (a, b) => ping(a) - ping(b),
  }
  ranked.sort((a, b) => order[priority](a.numbers, b.numbers))
  const first = ranked[0]?.numbers ?? null
  const pick = first && unknown > 0 && (ranked.length === 1 || !first.got5g) ? null : first
  return { rows: [...ranked, ...rest], pick }
}

/** What a finished or stopped scan found, for the summary above its table. */
export type ScanSummary = {
  noService: ScanResult[]
  /** The router refused them, or the speed test failed: neither says anything about the band. */
  inconclusive: ScanResult[]
  /** Worked without 5G, when the best has it: allowing these can cost the user 5G. */
  lose5g: ScanResult[]
  /** Fewest speed tests behind any speed shown. */
  tests: number
  bandsTried: number
  combosTried: number
}

export function summarizeScan(ranked: ScanResult[], pick: ScanCandidate | null): ScanSummary {
  const good = ranked.filter(measured)
  const others = good.filter((r) => r.bands !== pick?.bands)
  const size = (r: ScanResult) => supportedBands(r.bands).length
  return {
    noService: ranked.filter((r) => r.outcome === 'noService'),
    inconclusive: ranked.filter((r) => r.outcome === 'failed' || r.outcome === 'testFailed'),
    lose5g: pick?.got5g ? others.filter((r) => !r.got5g) : [],
    tests: good.length ? Math.min(...good.map((r) => r.tests)) : 0,
    bandsTried: ranked.filter((r) => size(r) === 1 && r.outcome !== 'failed').length,
    combosTried: ranked.filter((r) => size(r) > 1 && !r.isStart && r.outcome !== 'failed').length,
  }
}

/** The bands a scan was set up with, for running it again. Combinations came from its results. */
export function scanBands(queue: string[], addOns: number[] = []): number[] {
  return [...queue.map(supportedBands).flatMap((list) => (list.length === 1 ? list : [])), ...addOns]
}

/** Most combinations the user can ask for in one scan. Keep equal to Rust `scan::MAX_CUSTOM_SETS`. */
export const MAX_CUSTOM_COMBOS = 12

/** Thorough combines at most this many bands that worked, as Rust `MAX_COMBINED_BANDS` does. */
const MAX_WORKED = 4

/**
 * Up to 4 bands that worked here, to suggest combinations of: the last scan's single bands that
 * were measured, fastest download first, then bands whose past results are good, then slow ones,
 * each in band order. A downlink-only band can't carry a connection, so it's never one of them.
 */
export function workedBands(file: ScanFile | null, tones: Map<number, BandTone>, supported: number[]): number[] {
  const ok = (b: number) => supported.includes(b) && !isDownlinkOnly(b)
  const fromScan = (file?.results ?? [])
    .filter((r) => r.outcome === 'measured')
    .flatMap((r) => {
      const list = supportedBands(r.bands)
      return list.length === 1 && ok(list[0]) ? [{ band: list[0], down: r.downloadMbps ?? 0 }] : []
    })
    .sort((a, b) => b.down - a.down)
    .map((r) => r.band)
  const byTone = (tone: BandTone) =>
    [...tones.entries()]
      .filter(([b, t]) => t === tone && ok(b))
      .map(([b]) => b)
      .sort((a, b) => a - b)
  const out: number[] = []
  for (const b of [...fromScan, ...byTone('good'), ...byTone('slow')]) {
    if (out.length >= MAX_WORKED) break
    if (!out.includes(b)) out.push(b)
  }
  return out
}

/**
 * Every pair, then every triple, of the bands that worked (in band order), the same shape as
 * Thorough's own combinations. A set equal to the router's current bands is left out: the scan
 * always tests that one first.
 */
export function suggestCombos(worked: number[], current: string | null = null): number[][] {
  const top = [...new Set(worked)].filter((b) => !isDownlinkOnly(b)).slice(0, MAX_WORKED).sort((a, b) => a - b)
  const pairs: number[][] = []
  const triples: number[][] = []
  for (let i = 0; i < top.length; i++)
    for (let j = i + 1; j < top.length; j++) {
      pairs.push([top[i], top[j]])
      for (let k = j + 1; k < top.length; k++) triples.push([top[i], top[j], top[k]])
    }
  const now = current ? supportedBands(current).join(',') : null
  return [...pairs, ...triples].filter((c) => c.join(',') !== now)
}

/** Same bands, whatever the order: for comparing combinations. */
export const sameCombo = (a: number[], b: number[]): boolean => bandsToHex(a) === bandsToHex(b)

/** Seconds a config takes besides its speed test: reconnecting, then settling for 30 s. */
const CONFIG_S = 60 + 30
const TEST_S = { light: 45, full: 90, ookla: 60 } as const
/** Planning figures for one speed test, in MB; automatic tests use them too. */
export const TEST_MB = { light: 40, full: 200, ookla: 150 } as const

export type ScanEstimate = { configs: number; minutes: number; megabytes: number }

/** Thorough adds pairs and triples of up to 4 bands that worked, and each add-on band with each of them. */
function combinationCount(bands: number, addOns: number): number {
  const n = Math.min(bands, 4)
  return (n * (n - 1)) / 2 + (n * (n - 1) * (n - 2)) / 6 + addOns * n
}

/**
 * A planning figure, assuming every chosen band works. `custom` counts the combinations the user
 * asked for. A band with no service uses no data, and
 * costs a minute or so instead of a test. Thorough is a most-case figure, since its
 * combinations depend on which bands work.
 */
export function estimateScan(
  mode: ScanMode,
  bands: number,
  cfg: SpeedTestSettings,
  repeats = 1,
  addOns = 0,
  custom = 0,
): ScanEstimate {
  const kind = cfg.provider === 'ookla' && cfg.ooklaTermsAccepted ? 'ookla' : cfg.size
  const configs = 1 + bands + custom + (mode === 'thorough' ? combinationCount(bands, addOns) : 0)
  return {
    configs,
    minutes: Math.ceil((configs * (CONFIG_S + repeats * TEST_S[kind])) / 60),
    megabytes: configs * repeats * TEST_MB[kind],
  }
}
