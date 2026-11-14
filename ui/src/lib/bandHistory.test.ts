import { describe, expect, it } from 'vitest'
import { bandTones, lastBandRecord } from './bandHistory'
import { resultFixture } from './speedtest.fixtures'
import type { ScanResult } from './bindings/ScanResult'

const serving = { rat: 'lte', lteBand: 3, ltePci: 1, lteEarfcn: 1, lteCa: false, nrBand: null, nrPci: null, nrArfcn: null } as const

describe('lastBandRecord', () => {
  const test = (atMs: number, o = {}) => resultFixture({ atMs, serving, downloadMbps: 80, uploadMbps: 9, idlePingMs: 30, ...o } as never)

  it('has nothing to say about a band with no evidence', () => {
    expect(lastBandRecord(3, [], [], [], [])).toBeNull()
  })

  it('shows the newest speed test, and says when it ran inside a scan', () => {
    const r = lastBandRecord(3, [test(100), test(50, { downloadMbps: 1 })], [], [], [[90, 110]])
    expect(r).toMatchObject({ atMs: 100, kind: 'measured', fromScan: true, downloadMbps: 80, uploadMbps: 9, pingMs: 30, ltePci: 1 })
    expect(lastBandRecord(3, [test(100)], [], [], [[1, 2]])?.fromScan).toBe(false)
  })

  it('names the allowed set when the test was served on this band among others', () => {
    expect(lastBandRecord(3, [test(1, { allowedBands: '5' })], [], [], [])?.allowed).toBe('5')
    expect(lastBandRecord(3, [test(1, { allowedBands: '4' })], [], [], [])?.allowed).toBeNull()
  })

  it('a kept lock does not hide the speed test before it', () => {
    const r = lastBandRecord(3, [test(100)], [{ atMs: 200, band: 3, kind: 'kept' }], [], [])
    expect(r?.kind).toBe('measured')
  })

  it('a later no service wins, and a lock kept after it takes it back', () => {
    const dead = { atMs: 200, band: 3, kind: 'noService' } as const
    expect(lastBandRecord(3, [test(100)], [dead], [], [])).toMatchObject({ kind: 'noService', fromScan: false })
    const kept = { atMs: 300, band: 3, kind: 'kept' } as const
    expect(lastBandRecord(3, [test(100)], [dead, kept], [], [])?.kind).toBe('connected')
  })

  it('uses a scan of the band alone, skips one the router refused, and ignores other bands', () => {
    const row = (o: Partial<ScanResult>) => ({ atMs: 5, bands: '4', outcome: 'measured', got5g: true, nrBand: 41, lteBand: 3, ltePci: 7, downloadMbps: 60, uploadMbps: 6, idlePingMs: 20, ...o }) as ScanResult
    expect(lastBandRecord(3, [], [], [row({})], [])).toMatchObject({ fromScan: true, kind: 'measured', nrBand: 41 })
    expect(lastBandRecord(3, [], [], [row({ outcome: 'testFailed' })], [])?.kind).toBe('connected')
    expect(lastBandRecord(3, [], [], [row({ outcome: 'failed' })], [])).toBeNull()
    expect(lastBandRecord(3, [], [], [row({ bands: '5' })], [])).toBeNull()
  })
})

describe('a downlink-only band', () => {
  it('is not called dead for having no service alone', () => {
    expect(bandTones([], [{ atMs: 5, band: 32, kind: 'noService' }]).has(32)).toBe(false)
  })
})

describe('a scan\'s results', () => {
  const scanned = (bands: string, outcome: ScanResult['outcome'], atMs = 5) =>
    ({ bands, outcome, atMs, got5g: false, downloadMbps: outcome === 'measured' ? 50 : null }) as ScanResult

  it('tint a band tried alone right away, and say nothing about combinations', () => {
    expect(bandTones([], [], [scanned('1', 'noService')]).get(1)).toBe('noService')
    expect(bandTones([], [], [scanned('5', 'noService')]).size).toBe(0)
    expect(bandTones([], [], [scanned('8000000000', 'noService')]).get(40)).toBe('noService')
  })

  it('leave a band the router refused untinted, since it was never tried', () => {
    expect(bandTones([], [], [scanned('1', 'failed')]).has(1)).toBe(false)
  })

  it('replace older evidence', () => {
    expect(bandTones([resultFixture({ atMs: 1 })], [], [scanned('4', 'noService', 5)]).get(3)).toBe('noService')
  })
})

describe('bandTones', () => {
  it('leaves a band with no history out', () => {
    expect(bandTones([], []).size).toBe(0)
  })

  it('good means 5G or a good download; slow means neither', () => {
    expect(bandTones([resultFixture({ atMs: 1 })], []).get(3)).toBe('good')
    expect(bandTones([resultFixture({ atMs: 1, serving, downloadMbps: 40 })], []).get(3)).toBe('good')
    expect(bandTones([resultFixture({ atMs: 1, serving, downloadMbps: 2 })], []).get(3)).toBe('slow')
  })

  it('the newest evidence wins', () => {
    const noService = { atMs: 5, band: 3, kind: 'noService' } as const
    expect(bandTones([resultFixture({ atMs: 1 })], [noService]).get(3)).toBe('noService')
    expect(bandTones([resultFixture({ atMs: 9 })], [noService]).get(3)).toBe('good')
  })

  it('a later kept lock clears no service but adds no praise', () => {
    const list = [
      { atMs: 5, band: 28, kind: 'noService' },
      { atMs: 9, band: 28, kind: 'kept' },
    ] as const
    expect(bandTones([], [...list]).has(28)).toBe(false)
  })

  it('ignores a test where the band changed midway', () => {
    const r = resultFixture({ atMs: 1, invalidReason: 'bandChanged' } as never)
    expect(bandTones([r], []).size).toBe(0)
  })
})
