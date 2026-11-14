import { describe, expect, it } from 'vitest'
import { diagnose, fiveGReason, steady, MAIN_RULES, WEAK_RSRP, type DiagnosisInput } from './diagnosis'
import { THRESHOLDS } from './format'
import type { LteSignal } from './bindings/LteSignal'
import type { NrSignal } from './bindings/NrSignal'
import type { Status } from './bindings/Status'
import type { ConnState } from './bindings/ConnState'
import type { Sample } from './store.svelte'

const blankLte: LteSignal = {
  band: 3, earfcnDl: null, earfcnUl: null, pci: 103, cellId: null, enbId: null, sector: null,
  rsrp: null, rsrq: null, sinr: null, rssi: null, txPower: null, dlBwMhz: null, ulBwMhz: null,
  dlMcs: [null, null], ulMcs: null, cqi: [null, null], sccPci: null,
}
const lte = (o: Partial<LteSignal>): LteSignal => ({ ...blankLte, ...o })
const nr = (o: Partial<NrSignal> = {}): NrSignal => ({
  arfcnDl: 528980, arfcnUl: null, band: null, pci: 140, rsrp: -95, rsrq: -11, sinr: 11,
  txPower: null, dlBwMhz: null, ulBwMhz: null, dlMcs: [null, null], ulMcs: null, cqi: [null, null],
  ...o,
})
const status = (kind: ConnState['kind'] = 'connected', networkType = 101): Status => ({
  connection: kind === 'other' ? { kind, code: 905 } : ({ kind } as ConnState),
  networkType, operator: null, bars: null, nrBars: null,
})
const good = { rsrp: -85, rsrq: -9, sinr: 20 }

function run(o: {
  lte?: Partial<LteSignal> | null
  nr?: NrSignal | null
  status?: Status | null
  history?: Sample[]
  session?: DiagnosisInput['session']
}) {
  const l = o.lte === null ? null : lte(o.lte ?? good)
  const n = o.nr ?? null
  return diagnose({
    session: o.session,
    status: o.status === undefined ? status() : o.status,
    signal: { serving: l ? 'lte' : n ? 'nr' : null, lte: l, nr: n },
    history: o.history ?? [],
  })
}

describe('diagnose: connection rules', () => {
  it('waits without status', () => {
    expect(run({ status: null }).rule).toBe('waiting')
  })
  it('waits when not logged in', () => {
    expect(run({ session: { kind: 'connecting' } }).rule).toBe('waiting')
    expect(run({ session: { kind: 'loggedIn' } }).rule).toBe('good')
  })
  it('says so when nobody is logged in', () => {
    for (const kind of ['notConfigured', 'badCredentials', 'loggedOut'] as const)
      expect(run({ session: { kind } })).toMatchObject({ rule: 'loggedOut', main: { key: 'diagnosis.loggedOut' } })
    expect(run({ session: { kind: 'reconnecting', reason: 'x', retryInS: 5 } }).rule).toBe('waiting')
  })
  it('connecting', () => {
    expect(run({ status: status('connecting') }).main.key).toBe('diagnosis.connecting')
  })
  it('no service from the status', () => {
    const d = run({ status: status('noService') })
    expect(d.main.key).toBe('diagnosis.noService')
    expect(d.secondary).toBeNull()
  })
  it('no service when connected but no signal at all', () => {
    expect(run({ lte: null, nr: null }).rule).toBe('noService')
  })
  it('not connected for disconnected, failed and other', () => {
    for (const k of ['disconnected', 'failed', 'other'] as const)
      expect(run({ status: status(k) }).main.key).toBe('diagnosis.notConnected')
  })
  it('reading while RSRP is not readable (re-attach)', () => {
    expect(run({ lte: { rsrp: null } }).rule).toBe('reading')
  })
  it('connection rules win over weak signal', () => {
    expect(run({ status: status('noService'), lte: { rsrp: -120 } }).rule).toBe('noService')
  })
})

describe('diagnose: signal rules and boundaries', () => {
  it('weak below -100 dBm (the RSRP meter poor bound), not at -100', () => {
    expect(WEAK_RSRP).toBe(THRESHOLDS.rsrp[2])
    expect(run({ lte: { ...good, rsrp: -100.1 } }).rule).toBe('weak')
    expect(run({ lte: { ...good, rsrp: -103 } }).rule).toBe('weak')
    expect(run({ lte: { ...good, rsrp: -100 } }).rule).not.toBe('weak')
  })
  it('weak wins over low SINR and RSRQ', () => {
    expect(run({ lte: { rsrp: -110, sinr: -5, rsrq: -20 } }).rule).toBe('weak')
  })
  it('weak names the rat', () => {
    expect(run({ lte: { rsrp: -110 } }).main).toEqual({ key: 'diagnosis.weak', vars: { rat: '4G' } })
  })
  it('interference: RSRP >= -100 and SINR < 3', () => {
    expect(run({ lte: { rsrp: -100, rsrq: -9, sinr: 2.9 } }).rule).toBe('interference')
    expect(run({ lte: { rsrp: -100, rsrq: -9, sinr: 3 } }).rule).not.toBe('interference')
    expect(run({ lte: { rsrp: -100.5, rsrq: -9, sinr: -1 } }).rule).not.toBe('interference')
  })
  it('interference wins over congestion', () => {
    expect(run({ lte: { rsrp: -92, rsrq: -16, sinr: -1 } }).rule).toBe('interference')
  })
  it('congestion: RSRP >= -100 and RSRQ < -15', () => {
    expect(run({ lte: { rsrp: -100, rsrq: -15.5, sinr: 10 } }).rule).toBe('congestion')
    expect(run({ lte: { rsrp: -100, rsrq: -15, sinr: 10 } }).rule).not.toBe('congestion')
    expect(run({ lte: { rsrp: -101, rsrq: -18, sinr: 10 } }).rule).not.toBe('congestion')
  })
  it('good when every metric is good or better (inclusive)', () => {
    expect(run({ lte: { rsrp: -90, rsrq: -15, sinr: 13 } }).rule).toBe('good')
    expect(run({ lte: { rsrp: -85, rsrq: -9, sinr: 12.9 } }).rule).toBe('fair')
    expect(run({ lte: { rsrp: -90.1, rsrq: -9, sinr: 20 } }).rule).toBe('fair')
  })
  it('missing RSRQ or SINR does not block good', () => {
    expect(run({ lte: { rsrp: -80, rsrq: null, sinr: null } }).rule).toBe('good')
  })
  it('fair when strong but not good, without interference or congestion', () => {
    expect(run({ lte: { rsrp: -95, rsrq: -12, sinr: 5 } }).rule).toBe('fair')
  })
  it('the last rule always matches', () => {
    expect(MAIN_RULES.at(-1)!.id).toBe('fair')
  })
})

describe('diagnose: 5G secondary', () => {
  it('5G carries the connection when NR is present', () => {
    const d = run({ lte: { rsrp: -92, rsrq: -13, sinr: -1 }, nr: nr() })
    expect(d.main.key).toBe('diagnosis.interference')
    expect(d.secondary?.key).toBe('diagnosis.nrCarrying')
  })
  it('5G not available on LTE-only network types', () => {
    expect(run({ status: status('connected', 101) }).secondary?.key).toBe('diagnosis.nrUnavailable')
    expect(run({ status: status('connected', 1011) }).secondary?.key).toBe('diagnosis.nrUnavailable')
  })
  it('no 5G sentence on other network types', () => {
    expect(run({ status: status('connected', 111) }).secondary).toBeNull()
    expect(run({ status: status('connected', 46) }).secondary).toBeNull()
  })
  it('no secondary sentence without service', () => {
    expect(run({ status: status('disconnected'), nr: nr() }).secondary).toBeNull()
  })
  it('SA: NR is the anchor', () => {
    const d = run({ lte: null, nr: nr({ rsrp: -110 }), status: status('connected', 111) })
    expect(d.main).toEqual({ key: 'diagnosis.weak', vars: { rat: '5G' } })
    expect(d.secondary?.key).toBe('diagnosis.nrCarrying')
  })
})

describe('diagnose: history smoothing', () => {
  const hist = (rsrps: number[], sinrs?: number[]): Sample[] =>
    rsrps.map((r, i) => ({ t: 1_000_000 + i * 1000, lteRsrp: r, lteSinr: sinrs?.[i] ?? 20, nrRsrp: null, nrSinr: null }))
  it('one odd poll does not flip to weak', () => {
    const history = hist([-95, -95, -95, -95, -110])
    expect(run({ lte: { ...good, rsrp: -110 }, history }).rule).not.toBe('weak')
  })
  it('a sustained drop does', () => {
    const history = hist([-95, -110, -110, -110, -110])
    expect(run({ lte: { ...good, rsrp: -110 }, history }).rule).toBe('weak')
  })
  it('a null live value is not smoothed into a reading', () => {
    expect(run({ lte: { rsrp: null }, history: hist([-95, -95, -95]) }).rule).toBe('reading')
  })
})

describe('fiveGReason', () => {
  it('names the restriction first, then what the link says, and nothing when unknown', () => {
    expect(fiveGReason(true, true)).toBe('restricted')
    expect(fiveGReason(false, false)).toBe('off')
    expect(fiveGReason(true, false)).toBe('waiting')
    expect(fiveGReason(null, null)).toBeNull()
    expect(fiveGReason(undefined, undefined)).toBeNull()
  })
})

describe('steady', () => {
  const s = (vs: (number | null)[], step = 1000) => vs.map((v, i) => ({ t: i * step, v }))
  it('uses the live value with too little history', () => {
    expect(steady(-90, [])).toBe(-90)
    expect(steady(-90, s([-80, -80]))).toBe(-90)
  })
  it('median of the window, odd and even', () => {
    expect(steady(1, s([1, 5, 3]))).toBe(3)
    expect(steady(1, s([1, 2, 3, 4]))).toBe(2.5)
  })
  it('ignores nulls and samples outside 30 s', () => {
    expect(steady(1, s([100, 100, 100, 1, null, 2, 3], 10_000))).toBe(2)
  })
})
