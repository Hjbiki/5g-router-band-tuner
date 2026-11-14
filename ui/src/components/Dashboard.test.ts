import { afterEach, describe, expect, it, vi } from 'vitest'
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))
import { cleanup, fireEvent, render, screen } from '@testing-library/svelte'
import Dashboard from './Dashboard.svelte'
import { app } from '../lib/store.svelte'
import { history, resetHistory } from '../lib/history.svelte'
import { t } from '../lib/i18n'
import type { AppSnapshot } from '../lib/bindings/AppSnapshot'
import type { LteSignal } from '../lib/bindings/LteSignal'
import type { NrSignal } from '../lib/bindings/NrSignal'

const lte: LteSignal = {
  band: 3, earfcnDl: 1300, earfcnUl: null, pci: 103, cellId: null, enbId: null, sector: null,
  rsrp: -92, rsrq: -13, sinr: -1, rssi: null, txPower: null, dlBwMhz: 20, ulBwMhz: null,
  dlMcs: [null, null], ulMcs: null, cqi: [null, null], sccPci: null,
}
const nr: NrSignal = {
  arfcnDl: 528980, arfcnUl: null, band: { primary: 41, alternates: [], duplex: 'tdd', dlMhz: 2500 },
  pci: 140, rsrp: -95, rsrq: -11, sinr: 11, txPower: null, dlBwMhz: null, ulBwMhz: null,
  dlMcs: [null, null], ulMcs: null, cqi: [null, null],
}
const snap = (o: Partial<AppSnapshot> = {}): AppSnapshot => ({
  session: { kind: 'loggedIn' },
  device: null,
  status: { connection: { kind: 'connected' }, networkType: 101, operator: null, bars: null, nrBars: null },
  signal: { serving: 'lte', lte, nr },
  config: null,
  capabilities: null,
  safeApply: { phase: { kind: 'idle' }, secondsLeft: null, previous: null, requested: null },
  inService: true,
  foreignJournal: null,
  capturedAtMs: Date.now(),
  dataSwitchOff: false,
  ...o,
})

afterEach(() => {
  cleanup()
  app.snapshot = null
  app.tab = 'dashboard'
  app.history.splice(0)
})

describe('Dashboard', () => {
  it('shows the title and the diagnosis', () => {
    app.snapshot = snap()
    render(Dashboard)
    expect(screen.getByRole('heading', { level: 1, name: t('dashboard.title') })).toBeTruthy()
    const line = screen.getByTestId('diagnosis').textContent!
    expect(line).toContain(t('diagnosis.interference', { rat: '⁦4G⁩' }))
    expect(line).toContain(t('diagnosis.nrCarrying'))
  })
  it('says when the values were updated', () => {
    app.snapshot = snap({ capturedAtMs: Date.now() - 5_000 })
    render(Dashboard)
    expect(screen.getByTestId('updated').textContent).toBe(t('dashboard.updatedSeconds', { n: '5' }))
  })
  it('Change bands opens the Bands tab', async () => {
    app.snapshot = snap()
    render(Dashboard)
    await fireEvent.click(screen.getByRole('button', { name: t('dashboard.changeBands') }))
    expect(app.tab).toBe('bands')
  })
  it('shows both cards and says plainly when there is no 5G', () => {
    app.snapshot = snap({ signal: { serving: 'lte', lte, nr: null } })
    render(Dashboard)
    expect(screen.getByRole('region', { name: t('dashboard.lte') })).toBeTruthy()
    expect(screen.getByRole('region', { name: t('dashboard.nr') })).toBeTruthy()
    expect(screen.getByText(t('dashboard.noNr'))).toBeTruthy()
    expect(screen.getByTestId('diagnosis').textContent).toContain(t('diagnosis.nrUnavailable'))
  })
  it('with room to improve, links to the bands that have been fastest', async () => {
    resetHistory()
    app.snapshot = snap({ signal: { serving: 'lte', lte: { ...lte, sinr: 5 }, nr } })
    render(Dashboard)
    expect(screen.getByTestId('diagnosis').textContent).toContain(t('diagnosis.fair'))
    await fireEvent.click(screen.getByRole('button', { name: t('history.link.signal') }))
    expect(app.tab).toBe('history')
    expect(app.historyFocus).toBe('answer')
    expect(history.range).toEqual({ kind: '30d' })
    app.historyFocus = null
    resetHistory()
  })
  it('has no History link when the signal is good or another rule applies', () => {
    app.snapshot = snap()
    render(Dashboard)
    expect(screen.queryByTestId('history-link')).toBeNull()
  })
  it('asks for a login instead of showing empty signal cards', () => {
    app.snapshot = snap({ session: { kind: 'loggedOut' }, status: null, signal: null })
    render(Dashboard)
    expect(screen.getByTestId('diagnosis').textContent).toContain(t('diagnosis.loggedOut'))
    expect(screen.getByTestId('login-needed')).toBeTruthy()
    expect(screen.queryByText(t('dashboard.noLte'))).toBeNull()
  })
  it('offers what to try when there is no service', () => {
    const base = snap()
    app.snapshot = snap({
      status: { ...base.status!, connection: { kind: 'noService' } },
      signal: { lte: null, nr: null, serving: null },
    })
    render(Dashboard)
    expect(screen.getByTestId('no-service-help')).toBeTruthy()
  })
  it('keeps that advice away during a band change, when no service is expected', () => {
    const base = snap()
    app.snapshot = snap({
      status: { ...base.status!, connection: { kind: 'noService' } },
      signal: { lte: null, nr: null, serving: null },
      safeApply: { phase: { kind: 'reattachGrace' }, secondsLeft: 60, previous: null, requested: null },
    })
    render(Dashboard)
    expect(screen.queryByTestId('no-service-help')).toBeNull()
  })
  it('waits without a snapshot', () => {
    render(Dashboard)
    expect(screen.getByTestId('diagnosis').textContent).toContain(t('diagnosis.waiting'))
  })
})
