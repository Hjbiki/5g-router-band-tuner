import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'
import {
  LTE_BAND_INFO,
  bandChange,
  bandsOf,
  changeOpen,
  clock,
  bandsToHex,
  duplexOf,
  groupBands,
  isEmptyMask,
  splitDuration,
  supportedBands,
  templateParts,
  toggleBand,
  withBand,
} from './bands'
import type { NetConfig } from './bindings/NetConfig'

const cfg = (lteBands: string): NetConfig => ({
  networkMode: { kind: 'auto' },
  lteBands,
  legacyBandRaw: '0',
  nrOption: null,
})

describe('supportedBands', () => {
  it('decodes the router mask', () => {
    expect(supportedBands('7e2880800d5')).toEqual([1, 3, 5, 7, 8, 20, 28, 32, 34, 38, 39, 40, 41, 42, 43])
  })
  it('returns [] for garbage or empty', () => {
    expect(supportedBands('')).toEqual([])
    expect(supportedBands('zz')).toEqual([])
  })
})

describe('toggleBand matches Rust hex rules', () => {
  const hexAfter = (start: string, ...toggles: number[]) =>
    toggles.reduce((c, b) => toggleBand(c, b), cfg(start)).lteBands
  it('bookmarklet regression: 2 + band 4 is A', () => {
    expect(toggleBand(cfg('2'), 4).lteBands).toBe('A')
  })
  it('Task 2 regression cases', () => {
    expect(hexAfter('0', 2, 4)).toBe('A')
    expect(hexAfter('0', 3, 4)).toBe('C')
    expect(hexAfter('0', 1, 2, 3, 4)).toBe('F')
    expect(hexAfter('0', 38, 40)).toBe('A000000000')
    expect(hexAfter('0', 1, 3, 20)).toBe('80005')
  })
  it('uppercase, no leading zeros, "0" when empty', () => {
    expect(toggleBand(cfg('00ab'), 1).lteBands).toBe('AA')
    expect(toggleBand(cfg('1'), 1).lteBands).toBe('0')
  })
  it('toggling twice is the identity and the input is not mutated', () => {
    const c = cfg('5')
    const once = toggleBand(c, 7)
    expect(c.lteBands).toBe('5')
    expect(toggleBand(once, 7).lteBands).toBe('5')
  })
  it('last band off gives an empty mask', () => {
    expect(isEmptyMask(toggleBand(cfg('4'), 3))).toBe(true)
    expect(isEmptyMask(cfg('4'))).toBe(false)
  })
  it('keeps the other fields', () => {
    const c = { ...cfg('1'), legacyBandRaw: '3FFFFFFF', nrOption: 'sa' as const }
    expect(toggleBand(c, 3)).toMatchObject({ legacyBandRaw: '3FFFFFFF', nrOption: 'sa', networkMode: { kind: 'auto' } })
  })
})

describe('groupBands', () => {
  it('puts TDD bands in tdd and downlink-only bands in their own sdl group', () => {
    const g = groupBands([1, 3, 5, 7, 8, 20, 28, 29, 32, 34, 38, 39, 40, 41, 42, 43])
    expect(g.tdd).toEqual([34, 38, 39, 40, 41, 42, 43])
    expect(g.fdd).toEqual([1, 3, 5, 7, 8, 20, 28])
    expect(g.sdl).toEqual([29, 32])
  })
})

describe('duplexOf and bandsToHex', () => {
  it('names each band kind, and an unknown band counts as FDD', () => {
    expect(duplexOf(3)).toBe('fdd')
    expect(duplexOf(41)).toBe('tdd')
    expect(duplexOf(32)).toBe('sdl')
    expect(duplexOf(127)).toBe('fdd')
  })
  it('builds the same mask that supportedBands reads back', () => {
    expect(bandsToHex([1, 3, 20])).toBe('80005')
    expect(bandsToHex([])).toBe('0')
    expect(bandsToHex([0, 129, 3])).toBe('4')
    for (const list of [[1], [3, 41], [1, 3, 20, 28, 32], [128]]) {
      expect(supportedBands(bandsToHex(list))).toEqual(list)
    }
  })
})

describe('band list helpers', () => {
  it('bandsOf lists the bands of a config', () => {
    expect(bandsOf(cfg('80005'))).toEqual([1, 3, 20])
    expect(bandsOf(null)).toEqual([])
  })
  it('withBand adds a missing band and leaves a present one alone', () => {
    expect(withBand(cfg('4'), 1).lteBands).toBe('5')
    const c = cfg('5')
    expect(withBand(c, 3)).toBe(c)
  })
  it('bandChange compares the draft with the router config', () => {
    const now = cfg('5') // B1 + B3
    const draft = cfg('44') // B3 + B7
    expect(bandChange(draft, now, 1)).toBe('removed')
    expect(bandChange(draft, now, 3)).toBeNull()
    expect(bandChange(draft, now, 7)).toBe('added')
    expect(bandChange(draft, now, 20)).toBeNull()
    expect(bandChange(draft, null, 7)).toBeNull()
  })
})

describe('changeOpen', () => {
  const seq = { step: 1, total: 2, primary: 3, finalConfig: cfg('45'), origin: 'user' as const }
  it('is open while a safe-apply runs and closed when it has ended', () => {
    expect(changeOpen({ phase: { kind: 'probation' } })).toBe(true)
    expect(changeOpen({ phase: { kind: 'committed' } })).toBe(false)
    expect(changeOpen({ phase: { kind: 'reverted', reason: 'userRequested', fallback: false } })).toBe(false)
    expect(changeOpen({ phase: { kind: 'idle' }, sequence: null })).toBe(false)
  })
  it('stays open between the two steps of a primary-band apply', () => {
    expect(changeOpen({ phase: { kind: 'committed' }, sequence: seq })).toBe(true)
    expect(changeOpen({ phase: { kind: 'idle' }, sequence: seq })).toBe(true)
  })
  it('nothing loaded yet is not an open change', () => {
    expect(changeOpen(null)).toBe(false)
    expect(changeOpen(undefined)).toBe(false)
  })
})

describe('durations', () => {
  it('splits seconds into minutes and seconds', () => {
    expect(splitDuration(90)).toEqual({ m: 1, s: 30 })
    expect(splitDuration(180)).toEqual({ m: 3, s: 0 })
    expect(splitDuration(-5)).toEqual({ m: 0, s: 0 })
  })
  it('formats a countdown as m:ss', () => {
    expect(clock(52)).toBe('0:52')
    expect(clock(134)).toBe('2:14')
    expect(clock(600)).toBe('10:00')
  })
})

describe('templateParts', () => {
  it('splits text and slots in order', () => {
    expect(templateParts('Kept {bands}. Done.')).toEqual([{ text: 'Kept ' }, { slot: 'bands' }, { text: '. Done.' }])
    expect(templateParts('{a}{b}')).toEqual([{ slot: 'a' }, { slot: 'b' }])
    expect(templateParts('plain')).toEqual([{ text: 'plain' }])
  })
})

describe('LTE_BAND_INFO drift', () => {
  const src = readFileSync(resolve(__dirname, '../../../crates/router-core/src/bands/lte.rs'), 'utf8')
  const rust = new Map<number, { duplex: string; mhz: number }>()
  for (const m of src.matchAll(/\((\d+), (Fdd|Tdd|SdlOnly), \([\d.]+, [\d.]+\), (\d+)\)/g))
    rust.set(Number(m[1]), { duplex: m[2] === 'Fdd' ? 'fdd' : m[2] === 'Tdd' ? 'tdd' : 'sdl', mhz: Number(m[3]) })

  it('agrees with lte_band_meta for the router bands', () => {
    for (const b of [1, 3, 5, 7, 8, 20, 28, 32, 34, 38, 39, 40, 41, 42, 43]) {
      expect(rust.get(b), `rust B${b}`).toBeDefined()
      expect(LTE_BAND_INFO[b], `ts B${b}`).toEqual(rust.get(b))
    }
  })
  it('agrees for every band 1-53', () => {
    for (const [b, v] of rust) if (b <= 53) expect(LTE_BAND_INFO[b], `B${b}`).toEqual(v)
  })
})
