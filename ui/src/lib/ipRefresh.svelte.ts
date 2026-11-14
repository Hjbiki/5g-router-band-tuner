// Refresh IP in the UI: the dialog's open state, its input rules and the reasons it can't start.
// The backend (router-core `ip_refresh`) has the final say; these mirror its rules so the dialog
// can explain them before anything is sent.
import type { AppSnapshot } from './bindings/AppSnapshot'
import type { IpRefreshOutcome } from './bindings/IpRefreshOutcome'
import type { IpRefreshResult } from './bindings/IpRefreshResult'
import type { IpMethod } from './bindings/IpMethod'
import type { NetConfig } from './bindings/NetConfig'
import type { RouterError } from './bindings/RouterError'
import type { MessageKey } from './i18n'
import { bandsOf, bandsToHex, changeOpen, supportedBands } from './bands'
import { apnChangeOpen } from './apn'

/** The most tries one run may make, as in the backend. */
export const MAX_TRIES = 5
/** Data is held off this long on try 1, 2, 3, 4 and 5 (seconds). */
export const OFF_HOLDS_S = [5, 10, 20, 30, 30] as const
/** A rough time for the router to come back after each try, and the wait between tries. */
const RECONNECT_S = 30
const BETWEEN_S = 10
/** A network search: the router's answer and its reconnect take 1 to 3 minutes (the backend
 * allows more), so plan for the long end; the backend waits 20 s between searches. Five tries
 * still fit well under its 20-minute cap. */
const SEARCH_S = 180
const SEARCH_BETWEEN_S = 20

/** The dialog, mounted once in App. `prefill` is an address start from History, if any. */
export const ipRefreshUi: { open: boolean; prefill: string | null } = $state({ open: false, prefill: null })

/** Opens the Refresh IP dialog. Nothing starts until the user confirms in it. */
export function openIpRefresh(prefill: string | null = null): void {
  ipRefreshUi.prefill = prefill
  ipRefreshUi.open = true
}

export function closeIpRefresh(): void {
  ipRefreshUi.open = false
  ipRefreshUi.prefill = null
}

/** Arabic-Indic and Eastern Arabic digits, and the Arabic decimal separator, typed on an
 * Arabic keyboard, as ASCII. */
function asciiDigits(raw: string): string {
  return raw.replace(/[\u0660-\u0669\u06F0-\u06F9\u066B]/g, (c) => {
    if (c === '\u066B') return '.'
    const code = c.charCodeAt(0)
    return String(code >= 0x06f0 ? code - 0x06f0 : code - 0x0660)
  })
}

/** One or two numbers from 0 to 255 ("10" or "10.45"), as the backend accepts. Arabic digits
 * are read as the same numbers. */
export function normalizeTarget(raw: string): string | null {
  let s = asciiDigits(raw).trim()
  if (s.endsWith('.')) s = s.slice(0, -1)
  const parts = s.split('.')
  if (parts.length < 1 || parts.length > 2) return null
  for (const p of parts) {
    if (!/^[0-9]{1,3}$/.test(p) || Number(p) > 255) return null
  }
  return parts.map((p) => String(Number(p))).join('.')
}

/** The method a new run starts with: the driver's first. `null` hides Refresh IP. */
export function preferredMethod(s: AppSnapshot | null | undefined): IpMethod | null {
  return s?.capabilities?.ipMethods?.[0] ?? null
}

/** Roughly how long a run of `tries` takes, in seconds, when no try matches. */
export function estimateSeconds(tries: number, method: IpMethod = 'dataSwitch'): number {
  const n = Math.min(MAX_TRIES, Math.max(1, Math.round(tries)))
  if (method === 'networkSearch') return n * SEARCH_S + (n - 1) * SEARCH_BETWEEN_S
  let total = 0
  for (let i = 0; i < n; i++) total += OFF_HOLDS_S[i] + RECONNECT_S
  return total + (n - 1) * BETWEEN_S
}

export function ipRefreshActive(s: AppSnapshot | null | undefined): boolean {
  return s?.ipRefresh?.active === true
}

/** What holds the router, for the inline reason. `busy` names the UI's own scan or speed test. */
export function startRefusal(
  s: AppSnapshot | null | undefined,
  busy: { scan: boolean; speedTest: boolean },
): MessageKey | null {
  if (!s || s.session.kind !== 'loggedIn') return 'ipRefresh.why.notConnected'
  if (!s.capabilities?.ipMethods?.length) return 'ipRefresh.refuse.notSupported'
  if (ipRefreshActive(s)) return 'ipRefresh.refuse.running'
  if (s.ipRefresh?.journalBlocked) return 'ipRefresh.refuse.journalBlocked'
  if (changeOpen(s.safeApply)) return 'ipRefresh.why.bandChange'
  if (apnChangeOpen(s)) return 'ipRefresh.why.apnChange'
  if (busy.scan) return 'ipRefresh.why.scan'
  if (busy.speedTest) return 'ipRefresh.why.speedTest'
  if (s.ipRefresh && s.ipRefresh.cyclesLeft === 0) return 'ipRefresh.refuse.cooldown'
  if (!s.inService || !s.wan?.range) return 'ipRefresh.refuse.noService'
  return null
}

/** Outcomes that call for a look, shown and toasted as errors. */
const TROUBLE: ReadonlySet<IpRefreshOutcome> = new Set([
  'onlineTimeout',
  'hardCap',
  'sessionLost',
  'offRefused',
  'journalFailed',
  'dataOnFailed',
  'searchRefused',
])

export function outcomeIsTrouble(o: IpRefreshOutcome): boolean {
  return TROUBLE.has(o)
}

/** Outcomes whose words name mobile data, worded for a network search instead. */
const SEARCH_WORDING: ReadonlySet<IpRefreshOutcome> = new Set(['stopped', 'hardCap', 'sessionLost'])

export function outcomeKey(o: IpRefreshOutcome, method: IpMethod = 'dataSwitch'): MessageKey {
  if (method === 'networkSearch' && SEARCH_WORDING.has(o)) return `ipRefresh.outcomeSearch.${o}` as MessageKey
  return `ipRefresh.outcome.${o}` as MessageKey
}

/** The router's own error code in a refusal, when it sent one. */
export function refusalCode(e: RouterError | null | undefined): number | null {
  if (!e) return null
  if (e.kind === 'api' || e.kind === 'loginRejected') return e.code
  if (e.kind === 'sessionLost') return e.code ?? null
  return null
}

/** The LTE mask for "any band" (`LteBandSet::AUTO`). */
const AUTO_HEX = '7FFFFFFFFFFFFFFF'

export function isAutoBands(cfg: NetConfig): boolean {
  return bandsToHex(supportedBands(cfg.lteBands)) === AUTO_HEX
}

/**
 * The two steps of a band switch for a new IP, as the backend picks them (`reattach_step1`):
 * with 2 or more bands allowed, the serving band alone (else the lowest); with one band or
 * Automatic, every band the router supports. `null` when the backend would refuse.
 */
export function reattachPlan(s: AppSnapshot | null | undefined): { step1: NetConfig; final: NetConfig } | null {
  const config = s?.config
  const caps = s?.capabilities
  if (!config || !caps || !caps.bandEditAllowed) return null
  if (config.networkMode.kind !== 'auto' && config.networkMode.kind !== 'lteOnly') return null
  const auto = isAutoBands(config)
  const allowed = auto ? supportedBands(caps.supportedLte) : bandsOf(config)
  const serving = s?.signal?.lte?.band ?? null
  let step1Bands: number[]
  if (allowed.length >= 2) {
    step1Bands = [serving !== null && allowed.includes(serving) ? serving : allowed[0]]
  } else {
    step1Bands = supportedBands(caps.supportedLte)
  }
  const hex = bandsToHex(step1Bands)
  if (step1Bands.length === 0 || hex === bandsToHex(bandsOf(config)) || hex === bandsToHex(allowed)) return null
  return { step1: { ...config, lteBands: hex }, final: config }
}

/** Outcomes after which a band switch may still give a new range. */
const REATTACH_AFTER: ReadonlySet<IpRefreshOutcome> = new Set(['unchanged', 'notMatched', 'searchRefused', 'offRefused'])

/** Offer "Try a band switch": the run kept its range or was refused, and the bands can change now. */
export function offerReattach(result: IpRefreshResult | null | undefined, s: AppSnapshot | null | undefined): boolean {
  if (!result || !REATTACH_AFTER.has(result.outcome)) return false
  if (result.outcome !== 'offRefused' && result.method !== 'networkSearch') return false
  if (!s || s.session.kind !== 'loggedIn' || ipRefreshActive(s)) return false
  if (s.ipReattach?.active || changeOpen(s.safeApply) || apnChangeOpen(s)) return false
  return reattachPlan(s) !== null
}

/** Offer "Restart router": the internet didn't come back in time. */
export function offerReboot(result: IpRefreshResult | null | undefined): boolean {
  return result?.outcome === 'onlineTimeout'
}

/** Offer "Restore my bands": a search changed the band lock. Never done by itself. */
export function offerRestoreBands(result: IpRefreshResult | null | undefined): result is IpRefreshResult & { bandsBefore: NetConfig } {
  return result?.bandsChanged === true && result.bandsBefore != null
}
