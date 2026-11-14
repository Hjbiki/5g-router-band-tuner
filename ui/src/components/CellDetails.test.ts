import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/svelte'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))

import { t } from '../lib/i18n'
import CellDetails from './CellDetails.svelte'
import type { LteSignal } from '../lib/bindings/LteSignal'

const lte = {
  band: 3,
  pci: 103,
  earfcnDl: 1275,
  enbId: null,
  sector: null,
  dlBwMhz: 20,
  txPower: 14,
  dlMcs: [17, 12],
  ulMcs: 9,
  cqi: [9, 8],
} as unknown as LteSignal

describe('CellDetails', () => {
  afterEach(cleanup)

  it('explains every column, one at a time, under the table', async () => {
    render(CellDetails, { lte, nr: null })
    for (const name of ['PCI', t('dashboard.arfcn'), t('dashboard.enbSector'), t('dashboard.bandwidth'),
      t('metrics.uploadPower'), t('dashboard.mcs'), t('dashboard.cqi')])
      expect(screen.getByRole('button', { name: t('metrics.whatIs', { name }) })).toBeTruthy()

    await fireEvent.click(screen.getByRole('button', { name: t('metrics.whatIs', { name: 'PCI' }) }))
    expect(screen.getByRole('note').textContent).toContain(t('metrics.help.pci'))
    await fireEvent.click(screen.getByRole('button', { name: t('metrics.whatIs', { name: t('dashboard.cqi') }) }))
    expect(screen.getAllByRole('note')).toHaveLength(1)
    expect(screen.getByRole('note').textContent).toContain(t('metrics.help.cqi'))
  })

  it('shows one row per link and nothing without a signal', () => {
    const { container } = render(CellDetails, { lte, nr: null })
    expect(container.querySelectorAll('tbody tr')).toHaveLength(1)
    expect(container.textContent).toContain('B3')
    cleanup()
    const empty = render(CellDetails, { lte: null, nr: null })
    expect(empty.container.querySelector('table')).toBeNull()
  })
})
