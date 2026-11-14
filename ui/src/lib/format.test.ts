import { readFileSync } from 'node:fs'
import { t } from './i18n'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'
import {
  formatNumber,
  formatMeasure,
  qualityOf,
  bandLabel,
  THRESHOLDS,
  networkTypeLabel,
  formatFigure,
  qualitySteps,
  duplexLabel,
  updatedAgo,
} from './format'
import type { LteSignal } from './bindings/LteSignal'
import type { Signal } from './bindings/Signal'
import type { NrSignal } from './bindings/NrSignal'

describe('format', () => {
  it('formatNumber(null) === "—"', () => {
    expect(formatNumber(null)).toBe('—')
    expect(formatNumber(undefined)).toBe('—')
  })
  it('formatNumber(-95) === "-95"', () => expect(formatNumber(-95)).toBe('-95'))
  it('arabic-indic digits', () => {
    expect(formatNumber(-95, { digits: 'arabicIndic' })).toBe('-٩٥')
    expect(formatNumber(3.5, { digits: 'arabicIndic', fraction: 1 })).toBe('٣.٥')
  })
  it('formatMeasure', () => {
    expect(formatMeasure(3.25, 'dB')).toBe('3.3 dB')
    expect(formatMeasure(-95, 'dBm')).toBe('-95 dBm')
    expect(formatMeasure(20, 'MHz')).toBe('20 MHz')
    expect(formatMeasure(null, 'dB')).toBe('—')
  })
  it('qualityOf mirrors Rust thresholds', () => {
    expect(qualityOf('rsrp', -100)).toBe('fair')
    expect(qualityOf('rsrp', -80)).toBe('excellent')
    expect(qualityOf('rsrp', -100.1)).toBe('poor')
    expect(qualityOf('sinr', -0.5)).toBe('poor')
    expect(qualityOf('sinr', 13)).toBe('good')
    expect(qualityOf('rsrq', -15)).toBe('good')
    expect(qualityOf('rsrq', null)).toBeNull()
  })
  it('bandLabel', () => {
    const lte = { band: 3 } as LteSignal
    const nr = { band: { primary: 78, carriers: [78] } } as unknown as NrSignal
    expect(bandLabel(lte, nr)).toBe('B3 + n78')
    expect(bandLabel(lte, null)).toBe('B3')
    expect(bandLabel(null, null)).toBe('—')
  })
})

describe('thresholds drift', () => {
  it('quality thresholds match Rust', () => {
    const src = readFileSync(resolve(__dirname, '../../../crates/router-core/src/bands/quality.rs'), 'utf8')
    const grab = (m: string) => {
      const re = new RegExp(String.raw`Metric::${m}\s*=>\s*\[([^\]]+)\]`)
      const hit = re.exec(src)
      expect(hit, m).not.toBeNull()
      return hit![1].split(',').map((x) => Number(x.trim()))
    }
    // Probe the TS side through qualityOf boundaries: each Rust bound must classify as that tier.
    const tiers = ['excellent', 'good', 'fair'] as const
    for (const [m, key] of [['Rsrp', 'rsrp'], ['Rsrq', 'rsrq'], ['Sinr', 'sinr']] as const) {
      const bounds = grab(m)
      expect(bounds).toEqual(THRESHOLDS[key].slice())
      bounds.forEach((b, i) => {
        expect(qualityOf(key, b)).toBe(tiers[i])
        expect(qualityOf(key, b - 0.01)).toBe(i === 2 ? 'poor' : tiers[i + 1])
      })
    }
  })
})

describe('networkTypeLabel', () => {
  const sig = (lte: Partial<LteSignal> | null, nr: boolean): Signal =>
    ({ serving: 'lte', lte: lte && { sccPci: null, ...lte }, nr: nr ? {} : null }) as Signal
  const lteOnly = sig({}, false)

  it('maps router codes', () => {
    expect(networkTypeLabel(1011, lteOnly)).toBe(t('netType.lteCa'))
    expect(networkTypeLabel(111, null)).toBe(t('netType.nr'))
    expect(networkTypeLabel(42, null)).toBe(t('netType.hsdpa'))
    expect(networkTypeLabel(0, lteOnly)).toBe('—')
  })
  it('falls back to serving RAT then dash', () => {
    expect(networkTypeLabel(9999, lteOnly)).toBe(t('netType.lte'))
    expect(networkTypeLabel(null, null)).toBe('—')
  })
  it('upgrades plain LTE from the live signal (H122 reports 101 during CA and NSA)', () => {
    expect(networkTypeLabel(101, lteOnly)).toBe(t('netType.lte'))
    expect(networkTypeLabel(101, sig({ sccPci: 202 }, false))).toBe(t('netType.lteCa'))
    expect(networkTypeLabel(101, sig({ sccPci: 202 }, true))).toBe(
      t('netType.withNsa', { lte: t('netType.lteCa') }),
    )
    expect(networkTypeLabel(101, sig({}, true))).toBe(t('netType.withNsa', { lte: t('netType.lte') }))
    // Non-LTE codes are left alone.
    expect(networkTypeLabel(111, sig(null, true))).toBe(t('netType.nr'))
  })
})

describe('signal page helpers', () => {
  it('formatFigure matches formatMeasure without the unit', () => {
    for (const v of [-95, -92.46, 0, 13.04, null])
      expect(formatFigure(v)).toBe(v === null ? '—' : formatMeasure(v, 'dB').replace(' dB', ''))
    expect(formatFigure(-5.5, 'arabicIndic')).toBe('-٥.٥')
  })
  it('qualitySteps', () => {
    expect([qualitySteps('poor'), qualitySteps('fair'), qualitySteps('good'), qualitySteps('excellent'), qualitySteps(null)]).toEqual([1, 2, 3, 4, 0])
  })
  it('duplexLabel', () => {
    expect([duplexLabel('fdd'), duplexLabel('tdd'), duplexLabel('sdlOnly'), duplexLabel(null)]).toEqual(['FDD', 'TDD', 'SDL', '—'])
  })
  it('updatedAgo', () => {
    expect(updatedAgo(null, 0)).toBe('')
    expect(updatedAgo(10_000, 10_500)).toBe(t('dashboard.updatedNow'))
    expect(updatedAgo(10_000, 11_000)).toBe(t('dashboard.updatedSeconds', { n: '1' }))
    expect(updatedAgo(10_000, 69_999)).toBe(t('dashboard.updatedSeconds', { n: '59' }))
    expect(updatedAgo(10_000, 130_000)).toBe(t('dashboard.updatedMinutes', { n: '2' }))
    expect(updatedAgo(20_000, 10_000)).toBe(t('dashboard.updatedNow'))
  })
})
