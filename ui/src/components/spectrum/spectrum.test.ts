import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))
import { cleanup, fireEvent, render, screen } from '@testing-library/svelte'
import BandMap from './BandMap.svelte'
import Dashboard from '../Dashboard.svelte'
import { app } from '../../lib/store.svelte'
import { t } from '../../lib/i18n'
import { applyTheme } from '../../lib/theme'
import { settingsFixture } from '../../test-fixtures'
import type { AppSnapshot } from '../../lib/bindings/AppSnapshot'
import type { BandRecord } from '../../lib/bandHistory'
import type { NetConfig } from '../../lib/bindings/NetConfig'

const cfg: NetConfig = { networkMode: { kind: 'auto' }, lteBands: '5', legacyBandRaw: '0', nrOption: 'nsa' }
const record = (o: Partial<BandRecord>): BandRecord => ({
  atMs: 1, kind: 'measured', fromScan: false, downloadMbps: 100, uploadMbps: 10, pingMs: 30, got5g: true,
  nrBand: 41, lteBand: 1, ltePci: 101, allowed: null, ...o,
})

afterEach(() => {
  cleanup()
  app.snapshot = null
  app.settings = null
})

describe('the Design setting', () => {
  it('marks the page with the chosen design, and falls back to Classic', () => {
    const root = document.createElement('html')
    applyTheme('dark', 'carbon', 'spectrum', root)
    expect(root.dataset.design).toBe('spectrum')
    applyTheme('dark', 'carbon', 'neon' as never, root)
    expect(root.dataset.design).toBe('classic')
    applyTheme('dark', 'carbon', undefined, root)
    expect(root.dataset.design).toBe('classic')
  })
})

describe('BandMap', () => {
  const toggle = vi.fn()
  beforeEach(() => toggle.mockClear())
  const records = new Map<number, BandRecord | null>([
    [1, record({ downloadMbps: 180 })],
    [3, record({ downloadMbps: 90, got5g: false })],
    [28, record({ kind: 'noService', downloadMbps: null, got5g: false })],
    [20, null],
  ])
  const draw = (draft: NetConfig = cfg) =>
    render(BandMap, { supported: [1, 3, 20, 28], draft, current: cfg, serving: 1, records, onToggle: toggle })

  it('orders the bands from low frequency to high and marks the allowed ones', () => {
    draw()
    const cells = screen.getAllByRole('button').filter((b) => b.classList.contains('band'))
    expect(cells.map((b) => b.querySelector('.name')?.textContent)).toEqual(['B28', 'B20', 'B3', 'B1'])
    expect(cells.map((b) => b.getAttribute('aria-pressed'))).toEqual(['false', 'false', 'true', 'true'])
  })

  it('shows what each band gave last time, and which were never tried', () => {
    draw()
    const map = screen.getByTestId('band-map')
    expect(map.textContent).toContain('180')
    expect(map.textContent).toContain(t('spectrum.map.noService'))
    expect(map.textContent).toContain(t('spectrum.map.now'))
    // B1, B3 and B28 have a record; B20 has none.
    expect(map.textContent).toContain(t('spectrum.map.tried', { n: '⁦3⁩', total: '⁦4⁩' }))
  })

  it('toggles a band through the panel and says what the draft adds', async () => {
    draw({ ...cfg, lteBands: '80005' })
    expect(screen.getByTestId('band-map').textContent).toContain(t('spectrum.map.adding'))
    await fireEvent.click(screen.getByRole('button', { name: /^B28/ }))
    expect(toggle).toHaveBeenCalledWith(28)
  })
})

describe('Signal in the Spectrum design', () => {
  const snap = (): AppSnapshot =>
    ({
      session: { kind: 'loggedIn' },
      device: null,
      status: { connection: { kind: 'connected' }, networkType: 101, operator: null, bars: null, nrBars: null },
      signal: {
        serving: 'lte',
        lte: {
          band: 3, earfcnDl: 1300, earfcnUl: null, pci: 103, cellId: null, enbId: null, sector: null, rsrp: -92, rsrq: -13,
          sinr: 4, rssi: null, txPower: null, dlBwMhz: 20, ulBwMhz: null, dlMcs: [null, null], ulMcs: null, cqi: [null, null], sccPci: null,
        },
        nr: null,
      },
      config: null,
      capabilities: null,
      safeApply: { phase: { kind: 'idle' }, secondsLeft: null, previous: null, requested: null },
      inService: true,
      foreignJournal: null,
      capturedAtMs: Date.now(),
      dataSwitchOff: false,
    }) as AppSnapshot

  it('draws the link and the measurements table instead of the classic cards', () => {
    app.settings = settingsFixture({ design: 'spectrum' })
    app.snapshot = snap()
    render(Dashboard)
    expect(screen.getByTestId('link-picture')).toBeTruthy()
    expect(screen.getByText(t('spectrum.signalNow'))).toBeTruthy()
    // Interference has its own words, so a clear channel never reads "good interference".
    expect(screen.getByText(t('spectrum.interference.fair'))).toBeTruthy()
  })

  it('keeps the classic cards when Classic is chosen', () => {
    app.settings = settingsFixture()
    app.snapshot = snap()
    render(Dashboard)
    expect(screen.queryByTestId('link-picture')).toBeNull()
  })
})
