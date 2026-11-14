import type { ConnState } from './bindings/ConnState'
import type { NrSignal } from './bindings/NrSignal'
import type { SessionView } from './bindings/SessionView'
import type { Signal } from './bindings/Signal'
import type { Status } from './bindings/Status'
import type { Sample } from './store.svelte'
import type { MessageKey } from './i18n'
import { THRESHOLDS } from './format'
import { needsLogin } from './login'

/**
 * Rule-based, pure diagnosis of the live connection: one main sentence plus an optional
 * secondary one, as i18n keys with variables. Nothing here talks to the router.
 *
 * Main rules, first match wins (see MAIN_RULES):
 *  0. loggedOut     nobody is logged in and no login is on its way
 *  1. waiting       no snapshot yet, or not logged in, or no status yet
 *  2. connecting    the router reports it is connecting
 *  3. noService     no service, or the anchor has no signal at all
 *  4. notConnected  disconnected, failed or another status code
 *  5. reading       connected, but the anchor's RSRP isn't readable yet (re-attach)
 *  6. weak          anchor RSRP < -100 dBm (the RSRP meter's "poor"): the tower is far away or blocked
 *  7. interference  RSRP >= -100 dBm but SINR < 3 dB: nearby cells drown the signal
 *  8. congestion    RSRP >= -100 dBm but RSRQ < -15 dB: the tower is busy
 *  9. good          every readable anchor metric is good or excellent (qualityOf thresholds)
 * 10. fair          anything else: usable, with room to improve
 *
 * Secondary rules (only after rules 5-10, i.e. when connected with a signal), first match wins:
 *  a. nrCarrying     an NR leg is present
 *  b. nrUnavailable  no NR leg on an LTE-only network type (101 LTE, 1011 LTE CA)
 *
 * The anchor is LTE when present (NSA), else NR (SA). RSRP and SINR are the median of the
 * last 30 s of history so one odd poll doesn't flip the sentence; RSRQ isn't in the history,
 * so it is the live value. NR values are cached by this router for minutes, so they are not
 * judged. To add a rule (for example router temperature, which this firmware doesn't expose as
 * far as we know), add a field to `DiagnosisInput` and `Ctx`, fill it in `context`, and insert
 * a rule in MAIN_RULES at the priority it deserves.
 */

export type Line = { key: MessageKey; vars?: Record<string, string | number> }
export type Diagnosis = { rule: string; main: Line; secondary: Line | null }

export type DiagnosisInput = {
  session?: SessionView | null
  status: Status | null | undefined
  signal: Signal | null | undefined
  history: readonly Sample[]
}

/**
 * Below this LTE/NR RSRP (dBm) the signal is weak. The same bound as the RSRP meter's "poor"
 * and the RSRP help text, so the sentence and the meter never disagree.
 */
export const WEAK_RSRP = THRESHOLDS.rsrp[2]
/** At or above this RSRP (dBm) the signal counts as strong for the interference/busy rules. */
export const STRONG_RSRP = -100
/** Below this SINR (dB) a strong signal is suffering from interference. */
export const INTERFERENCE_SINR = 3
/** Below this RSRQ (dB) a strong signal points at a busy (congested) tower. */
export const CONGESTION_RSRQ = -15
/** How far back the history smoothing looks. */
export const STEADY_WINDOW_MS = 30_000
/** Network type codes that mean LTE without NR. */
const LTE_ONLY_TYPES = new Set([101, 1011])

type Anchor = { rat: '4G' | '5G'; rsrp: number | null; rsrq: number | null; sinr: number | null }
type Ctx = {
  needsLogin: boolean
  loggedIn: boolean
  connection: ConnState | null
  networkType: number | null
  anchor: Anchor | null
  nr: NrSignal | null
}
type Rule = { id: string; when: (c: Ctx) => boolean; line: (c: Ctx) => Line }

const isNum = (v: number | null | undefined): v is number => typeof v === 'number' && Number.isFinite(v)

/** Median of the recent history values, or the live value if the history has too little. */
export function steady(live: number | null, series: readonly { t: number; v: number | null }[]): number | null {
  if (!isNum(live)) return null
  const last = series.at(-1)
  if (!last) return live
  const recent = series.filter((s) => s.t >= last.t - STEADY_WINDOW_MS && isNum(s.v)).map((s) => s.v as number)
  if (recent.length < 3) return live
  recent.sort((a, b) => a - b)
  const mid = recent.length >> 1
  return recent.length % 2 ? recent[mid] : (recent[mid - 1] + recent[mid]) / 2
}

function context(input: DiagnosisInput): Ctx {
  const { signal, history } = input
  const lte = signal?.lte ?? null
  const nr = signal?.nr ?? null
  let anchor: Anchor | null = null
  if (lte) {
    anchor = {
      rat: '4G',
      rsrp: steady(lte.rsrp, history.map((s) => ({ t: s.t, v: s.lteRsrp }))),
      rsrq: isNum(lte.rsrq) ? lte.rsrq : null,
      sinr: steady(lte.sinr, history.map((s) => ({ t: s.t, v: s.lteSinr }))),
    }
  } else if (nr) {
    anchor = {
      rat: '5G',
      rsrp: steady(nr.rsrp, history.map((s) => ({ t: s.t, v: s.nrRsrp }))),
      rsrq: isNum(nr.rsrq) ? nr.rsrq : null,
      sinr: steady(nr.sinr, history.map((s) => ({ t: s.t, v: s.nrSinr }))),
    }
  }
  return {
    needsLogin: needsLogin(input.session),
    loggedIn: input.session == null || input.session.kind === 'loggedIn',
    connection: input.status?.connection ?? null,
    networkType: input.status?.networkType ?? null,
    anchor,
    nr,
  }
}

const strong = (c: Ctx) => isNum(c.anchor?.rsrp) && c.anchor.rsrp >= STRONG_RSRP
const ratVars = (c: Ctx) => ({ rat: c.anchor?.rat ?? '4G' })
const goodOrBetter = { rsrp: THRESHOLDS.rsrp[1], rsrq: THRESHOLDS.rsrq[1], sinr: THRESHOLDS.sinr[1] }

export const MAIN_RULES: readonly Rule[] = [
  { id: 'loggedOut', when: (c) => c.needsLogin, line: () => ({ key: 'diagnosis.loggedOut' }) },
  { id: 'waiting', when: (c) => !c.loggedIn || !c.connection, line: () => ({ key: 'diagnosis.waiting' }) },
  {
    id: 'connecting',
    when: (c) => c.connection?.kind === 'connecting',
    line: () => ({ key: 'diagnosis.connecting' }),
  },
  {
    id: 'noService',
    when: (c) => c.connection?.kind === 'noService' || (c.connection?.kind === 'connected' && !c.anchor),
    line: () => ({ key: 'diagnosis.noService' }),
  },
  {
    id: 'notConnected',
    when: (c) => c.connection?.kind !== 'connected',
    line: () => ({ key: 'diagnosis.notConnected' }),
  },
  { id: 'reading', when: (c) => !isNum(c.anchor?.rsrp), line: () => ({ key: 'diagnosis.reading' }) },
  {
    id: 'weak',
    when: (c) => isNum(c.anchor?.rsrp) && c.anchor.rsrp < WEAK_RSRP,
    line: (c) => ({ key: 'diagnosis.weak', vars: ratVars(c) }),
  },
  {
    id: 'interference',
    when: (c) => strong(c) && isNum(c.anchor?.sinr) && c.anchor.sinr < INTERFERENCE_SINR,
    line: (c) => ({ key: 'diagnosis.interference', vars: ratVars(c) }),
  },
  {
    id: 'congestion',
    when: (c) => strong(c) && isNum(c.anchor?.rsrq) && c.anchor.rsrq < CONGESTION_RSRQ,
    line: (c) => ({ key: 'diagnosis.congestion', vars: ratVars(c) }),
  },
  {
    id: 'good',
    when: (c) =>
      (['rsrp', 'rsrq', 'sinr'] as const).every((m) => {
        const v = c.anchor?.[m]
        return !isNum(v) || v >= goodOrBetter[m]
      }),
    line: () => ({ key: 'diagnosis.good' }),
  },
  { id: 'fair', when: () => true, line: () => ({ key: 'diagnosis.fair' }) },
]

/** Main rules after which there is a live signal worth a secondary sentence. */
const WITH_SIGNAL = new Set(['reading', 'weak', 'interference', 'congestion', 'good', 'fair'])

export const SECONDARY_RULES: readonly Rule[] = [
  { id: 'nrCarrying', when: (c) => c.nr !== null, line: () => ({ key: 'diagnosis.nrCarrying' }) },
  {
    id: 'nrUnavailable',
    when: (c) => c.nr === null && c.networkType !== null && LTE_ONLY_TYPES.has(c.networkType),
    line: () => ({ key: 'diagnosis.nrUnavailable' }),
  },
]

export function diagnose(input: DiagnosisInput): Diagnosis {
  const c = context(input)
  const rule = MAIN_RULES.find((r) => r.when(c))!
  const second = WITH_SIGNAL.has(rule.id) ? SECONDARY_RULES.find((r) => r.when(c)) : undefined
  return { rule: rule.id, main: rule.line(c), secondary: second ? second.line(c) : null }
}

/** Why there's no 5G, by what the router says: restricted, not linked, or linked but not joined. */
export type FiveGReason = 'restricted' | 'off' | 'waiting'

export function fiveGReason(endc: boolean | null | undefined, restricted: boolean | null | undefined): FiveGReason | null {
  if (restricted === true) return 'restricted'
  if (endc === false) return 'off'
  if (endc === true) return 'waiting'
  return null
}
