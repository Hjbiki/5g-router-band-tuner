import { afterEach, describe, expect, it } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/svelte'
import SignalCard from './SignalCard.svelte'
import { t } from '../lib/i18n'
import type { LteSignal } from '../lib/bindings/LteSignal'
import type { NrSignal } from '../lib/bindings/NrSignal'
import type { Sample } from '../lib/store.svelte'

const reattach: LteSignal = {
  band: null, earfcnDl: null, earfcnUl: null, pci: null, cellId: null, enbId: null, sector: null,
  rsrp: null, rsrq: null, sinr: null, rssi: null, txPower: null, dlBwMhz: null, ulBwMhz: null,
  dlMcs: [null, null], ulMcs: null, cqi: [null, null], sccPci: null,
}
const lte = (o: Partial<LteSignal>): LteSignal => ({ ...reattach, ...o })
const nr = (o: Partial<NrSignal>): NrSignal => ({
  arfcnDl: null, arfcnUl: null, band: null, pci: null, rsrp: null, rsrq: null, sinr: null,
  txPower: null, dlBwMhz: null, ulBwMhz: null, dlMcs: [null, null], ulMcs: null, cqi: [null, null],
  ...o,
})
const sample = (i: number, lteSinr: number | null): Sample => ({
  t: 1_000_000 + i * 1000, lteSinr, lteRsrp: -90, nrSinr: null, nrRsrp: null,
})

afterEach(cleanup)

describe('no 5G', () => {
  afterEach(cleanup)
  it('says why when the router explains it, and gives the general hint otherwise', () => {
    render(SignalCard, { rat: 'nr', signal: null, history: [], noFiveG: 'restricted' })
    expect(screen.getByTestId('no-5g-reason').textContent).toBe(t('dashboard.noNrReason.restricted'))
    cleanup()
    render(SignalCard, { rat: 'nr', signal: null, history: [], paired: true })
    expect(screen.getByTestId('no-5g-reason').textContent).toBe(t('dashboard.noNrHint'))
  })
  it('does not say the router is on 4G when there is no 4G either', () => {
    render(SignalCard, { rat: 'nr', signal: null, history: [], paired: false })
    expect(screen.queryByTestId('no-5g-reason')).toBeNull()
  })
})

describe('SignalCard', () => {
  it('renders dashes for nulls', () => {
    const { container } = render(SignalCard, { rat: 'lte', signal: reattach, history: [] })
    expect(screen.getAllByText('—').length).toBeGreaterThan(3)
    for (const id of ['rsrp', 'rsrq', 'sinr'])
      expect(container.querySelector(`[data-testid="${id}"]`)!.textContent).toBe('—')
    expect(container.textContent).not.toMatch(/(^|[^\d.-])0(\s|$)/)
  })
  it('shows quality text', () => {
    render(SignalCard, { rat: 'lte', signal: lte({ rsrp: -100 }), history: [] })
    expect(screen.getByText(t('quality.fair'))).toBeTruthy()
  })
  it('fills the meter steps by quality', () => {
    const { container } = render(SignalCard, {
      rat: 'lte',
      signal: lte({ rsrp: -79, rsrq: -12, sinr: -1 }),
      history: [],
    })
    const meters = [...container.querySelectorAll('.meter')].map((m) => m.querySelectorAll('i.on').length)
    expect(meters).toEqual([4, 3, 1])
    expect(screen.getByText(t('quality.excellent'))).toBeTruthy()
    expect(screen.getByText(t('quality.good'))).toBeTruthy()
    expect(screen.getByText(t('quality.poor'))).toBeTruthy()
  })
  it('wraps values in bdi', () => {
    const { container } = render(SignalCard, { rat: 'lte', signal: lte({ rsrp: -95 }), history: [] })
    const el = container.querySelector('[data-testid="rsrp"]')!
    expect(el.closest('bdi[dir="ltr"]')).not.toBeNull()
    expect(el.textContent).toContain('-95')
    expect(el.closest('bdi')!.textContent).toContain('dBm')
  })
  it('shows the band with MHz, duplex and PCI', () => {
    const { container } = render(SignalCard, { rat: 'lte', signal: lte({ band: 3, pci: 103 }), history: [] })
    expect(container.querySelector('[data-testid="band"]')!.textContent).toBe('B3')
    expect(container.querySelector('.meta')!.textContent).toBe('1800 MHz · FDD · PCI 103')
  })
  it('nr band tooltip lists alternates', () => {
    const { container } = render(SignalCard, {
      rat: 'nr',
      signal: nr({ band: { primary: 78, alternates: [77], duplex: 'tdd', dlMhz: 3500 } }),
      history: [],
    })
    const el = container.querySelector('[data-testid="band"]')!
    expect(el.closest('[title]')!.getAttribute('title')).toContain('n77')
  })
  it('explains each metric on request', async () => {
    render(SignalCard, { rat: 'lte', signal: lte({ sinr: 5 }), history: [] })
    const btn = screen.getByRole('button', { name: t('metrics.whatIs', { name: 'SINR' }) })
    expect(btn.getAttribute('aria-expanded')).toBe('false')
    await fireEvent.click(btn)
    expect(btn.getAttribute('aria-expanded')).toBe('true')
    expect(screen.getByRole('note').textContent).toContain('13 dB')
  })
  it('draws the SINR chart from history', () => {
    const history = [0, 1, 2, 3].map((i) => sample(i, i - 1))
    const { container } = render(SignalCard, { rat: 'lte', signal: lte({ sinr: 2 }), history })
    const svg = container.querySelector('svg[role="img"]')!
    expect(svg.getAttribute('aria-label')).toContain('4G')
    expect(container.querySelector('polyline')).not.toBeNull()
  })
  it('keeps the 5G caching note in a tip on the NR card only', async () => {
    const label = t('metrics.whatIs', { name: 'SINR' })
    render(SignalCard, { rat: 'nr', signal: nr({ rsrp: -95 }), history: [] })
    expect(screen.queryByText(t('dashboard.nrCached'))).toBeNull()
    const tips = screen.getAllByRole('button', { name: label })
    await fireEvent.click(tips[tips.length - 1])
    expect(screen.getByText(t('dashboard.nrCached'))).toBeTruthy()
    cleanup()
    render(SignalCard, { rat: 'lte', signal: reattach, history: [] })
    for (const tip of screen.getAllByRole('button', { name: label })) await fireEvent.click(tip)
    expect(screen.queryByText(t('dashboard.nrCached'))).toBeNull()
  })
  it('null NR signal says there is no 5G in plain words', () => {
    render(SignalCard, { rat: 'nr', signal: null, history: [], paired: true })
    expect(screen.getByText(t('dashboard.noNr'))).toBeTruthy()
    expect(screen.getByText(t('dashboard.noNrHint'))).toBeTruthy()
  })
  it('null LTE signal says there is no 4G', () => {
    render(SignalCard, { rat: 'lte', signal: null, history: [] })
    expect(screen.getByText(t('dashboard.noLte'))).toBeTruthy()
  })
  it('names the leg role', () => {
    render(SignalCard, { rat: 'lte', signal: lte({}), history: [], paired: true })
    expect(screen.getByText(t('dashboard.lteAnchor'))).toBeTruthy()
  })
})
