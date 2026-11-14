import { describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))

import {
  estimateSeconds,
  isAutoBands,
  offerReattach,
  offerReboot,
  offerRestoreBands,
  outcomeKey,
  preferredMethod,
  reattachPlan,
  refusalCode,
} from './ipRefresh.svelte'
import { bandsOf, supportedBands } from './bands'
import { SUPPORTED_HEX, snap } from '../test-fixtures'
import type { AppSnapshot } from './bindings/AppSnapshot'
import type { IpRefreshResult } from './bindings/IpRefreshResult'
import type { NetConfig } from './bindings/NetConfig'

const B1_B3: NetConfig = { networkMode: { kind: 'auto' }, lteBands: '5', legacyBandRaw: '0', nrOption: 'saNsa' }
const B3: NetConfig = { ...B1_B3, lteBands: '4' }
const AUTO: NetConfig = { ...B1_B3, lteBands: '7FFFFFFFFFFFFFFF' }

const withServing = (band: number | null, o: Partial<AppSnapshot> = {}): AppSnapshot =>
  snap({ signal: { serving: 'lte', lte: { band } as never, nr: null }, wan: { ipv4: null, range: '10.45', kind: 'private', ipv6: false }, ...o })

const result = (o: Partial<IpRefreshResult> = {}): IpRefreshResult => ({
  outcome: 'unchanged',
  tries: 1,
  target: null,
  from: '10.45',
  to: '10.45',
  method: 'networkSearch',
  refusal: null,
  bandsChanged: false,
  bandsBefore: null,
  ...o,
})

describe('Refresh IP methods', () => {
  it('takes the driver’s first method', () => {
    const s = snap()
    expect(preferredMethod(s)).toBeNull()
    s.capabilities = { ...s.capabilities!, ipMethods: ['networkSearch', 'dataSwitch'] }
    expect(preferredMethod(s)).toBe('networkSearch')
  })

  it('estimates up to about 3 minutes per search, with 20 s between them', () => {
    expect(estimateSeconds(1, 'networkSearch')).toBe(180)
    expect(estimateSeconds(3, 'networkSearch')).toBe(3 * 180 + 2 * 20)
    expect(estimateSeconds(9, 'networkSearch')).toBe(estimateSeconds(5, 'networkSearch'))
    // The data switch keeps its own estimate.
    expect(estimateSeconds(1, 'dataSwitch')).toBe(35)
    expect(estimateSeconds(1)).toBe(35)
  })

  it('words the outcomes that name mobile data for a search', () => {
    expect(outcomeKey('stopped', 'networkSearch')).toBe('ipRefresh.outcomeSearch.stopped')
    expect(outcomeKey('hardCap', 'networkSearch')).toBe('ipRefresh.outcomeSearch.hardCap')
    expect(outcomeKey('stopped', 'dataSwitch')).toBe('ipRefresh.outcome.stopped')
    expect(outcomeKey('unchanged', 'networkSearch')).toBe('ipRefresh.outcome.unchanged')
  })

  it('reads the router’s code from a refusal', () => {
    expect(refusalCode({ kind: 'sessionLost', code: 100003 })).toBe(100003)
    expect(refusalCode({ kind: 'sessionLost' })).toBeNull()
    expect(refusalCode({ kind: 'api', code: 9003, errorKind: 'other' } as never)).toBe(9003)
    expect(refusalCode({ kind: 'timeout' })).toBeNull()
    expect(refusalCode(null)).toBeNull()
  })
})

describe('band switch plan (as the backend picks it)', () => {
  it('keeps only the serving band out of two or more', () => {
    const p = reattachPlan(withServing(3, { config: B1_B3 }))
    expect(bandsOf(p?.step1)).toEqual([3])
    expect(p?.final).toEqual(B1_B3)
    // A serving band outside the set, or none: the lowest band.
    expect(bandsOf(reattachPlan(withServing(20, { config: B1_B3 }))?.step1)).toEqual([1])
    expect(bandsOf(reattachPlan(withServing(null, { config: B1_B3 }))?.step1)).toEqual([1])
  })

  it('widens one band to every supported band, and Automatic to the serving one', () => {
    expect(bandsOf(reattachPlan(withServing(3, { config: B3 }))?.step1)).toEqual(supportedBands(SUPPORTED_HEX))
    expect(isAutoBands(AUTO)).toBe(true)
    expect(isAutoBands(B1_B3)).toBe(false)
    expect(bandsOf(reattachPlan(withServing(3, { config: AUTO }))?.step1)).toEqual([3])
  })

  it('has no plan without 4G, band editing or other bands', () => {
    expect(reattachPlan(withServing(3, { config: { ...B1_B3, networkMode: { kind: 'nrOnly' } } }))).toBeNull()
    const s = withServing(3, { config: B1_B3 })
    s.capabilities = { ...s.capabilities!, bandEditAllowed: false }
    expect(reattachPlan(s)).toBeNull()
    const only = withServing(3, { config: B3 })
    only.capabilities = { ...only.capabilities!, supportedLte: '4' }
    expect(reattachPlan(only)).toBeNull()
  })
})

describe('what the banner offers after a run', () => {
  const s = withServing(3, { config: B1_B3 })

  it('offers a band switch after a search that kept the range or was refused, and after a refused "off"', () => {
    for (const outcome of ['unchanged', 'notMatched', 'searchRefused'] as const) {
      expect(offerReattach(result({ outcome }), s), outcome).toBe(true)
    }
    expect(offerReattach(result({ outcome: 'offRefused', method: 'dataSwitch' }), s)).toBe(true)
    expect(offerReattach(result({ outcome: 'unchanged', method: 'dataSwitch' }), s)).toBe(false)
    for (const outcome of ['changed', 'matched', 'onlineTimeout', 'stopped'] as const) {
      expect(offerReattach(result({ outcome }), s), outcome).toBe(false)
    }
  })

  it('does not offer it while bands are changing or can’t change', () => {
    const busy = withServing(3, {
      config: B1_B3,
      safeApply: { phase: { kind: 'probation' }, secondsLeft: 100, previous: null, requested: null },
    })
    expect(offerReattach(result(), busy)).toBe(false)
    const running = withServing(3, { config: B1_B3, ipReattach: { active: true, fromRange: '10.45', last: null } })
    expect(offerReattach(result(), running)).toBe(false)
    const locked = withServing(3, { config: B1_B3 })
    locked.capabilities = { ...locked.capabilities!, bandEditAllowed: false }
    expect(offerReattach(result(), locked)).toBe(false)
    expect(offerReattach(result(), withServing(3, { config: B1_B3, session: { kind: 'loggedOut' } }))).toBe(false)
  })

  it('offers a restart only when the internet did not come back, and the bands only when they changed', () => {
    expect(offerReboot(result({ outcome: 'onlineTimeout' }))).toBe(true)
    expect(offerReboot(result())).toBe(false)
    expect(offerRestoreBands(result({ bandsChanged: true, bandsBefore: B3 }))).toBe(true)
    expect(offerRestoreBands(result({ bandsChanged: true, bandsBefore: null }))).toBe(false)
    expect(offerRestoreBands(result({ bandsChanged: false, bandsBefore: B3 }))).toBe(false)
    expect(offerRestoreBands(null)).toBe(false)
  })
})
