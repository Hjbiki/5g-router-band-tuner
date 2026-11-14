import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))
vi.mock('../../lib/backend', () => ({
  api: { exportHistoryCsv: vi.fn() },
  onHistoryCleared: vi.fn(),
}))

import { api } from '../../lib/backend'
import { app } from '../../lib/store.svelte'
import { setLanguage, t } from '../../lib/i18n'
import { history, resetHistory } from '../../lib/history.svelte'
import { settingsFixture } from '../../test-fixtures'
import HistoryExport from './HistoryExport.svelte'

const RANGE = { fromMs: 1_700_000_000_000, toMs: 1_700_086_400_000, utcOffsetMin: 180 }

describe('HistoryExport', () => {
  beforeEach(() => {
    resetHistory()
    history.loadedRange = RANGE
    app.settings = settingsFixture()
    app.toasts = []
  })
  afterEach(() => {
    cleanup()
    vi.clearAllMocks()
    setLanguage('en')
  })

  it('starts on the hourly summary with the cell numbers off', () => {
    render(HistoryExport)
    expect((screen.getByLabelText(new RegExp(t('history.export.dataset.hourly'))) as HTMLInputElement).checked).toBe(true)
    expect((screen.getByRole('checkbox', { name: new RegExp(t('history.export.cellNumbers').replace(/[()]/g, '\\$&')) }) as HTMLInputElement).checked).toBe(false)
    expect(screen.getByText(t('history.export.cellNumbersHint'))).toBeTruthy()
  })

  it('offers English column names only when the app is in another language', () => {
    render(HistoryExport)
    expect(screen.queryByText(t('history.export.english'))).toBeNull()
    cleanup()
    setLanguage('ar')
    render(HistoryExport)
    expect(screen.getByText(t('history.export.english'))).toBeTruthy()
  })

  it('sends the dataset, the range and the boxes, and reports the saved files', async () => {
    vi.mocked(api.exportHistoryCsv).mockResolvedValue({ kind: 'saved', files: ['a.csv', 'b.csv'] })
    setLanguage('ar')
    const onclose = vi.fn()
    render(HistoryExport, { onclose })
    await fireEvent.click(screen.getByLabelText(new RegExp(t('history.export.dataset.all'))))
    await fireEvent.click(screen.getByText(t('history.export.cellNumbers')))
    await fireEvent.click(screen.getByText(t('history.export.ipRange')))
    await fireEvent.click(screen.getByText(t('history.export.english')))
    await fireEvent.click(screen.getByRole('button', { name: t('history.export.chooseFolder') }))
    await waitFor(() => expect(api.exportHistoryCsv).toHaveBeenCalledOnce())
    const [opts, labels] = vi.mocked(api.exportHistoryCsv).mock.calls[0]
    expect(opts).toEqual({ range: RANGE, dataset: 'all', includeCellNumbers: true, includeIpRange: true })
    // English headers although the app is in Arabic.
    expect(labels.headers.localTime).toBe('Local time ({offset})')
    await waitFor(() => expect(app.toasts.map((x) => x.text)).toContain(t('history.export.saved', { files: 'a.csv, b.csv' })))
    expect(onclose).toHaveBeenCalledOnce()
  })

  it('uses the app language for the headers unless asked otherwise', async () => {
    vi.mocked(api.exportHistoryCsv).mockResolvedValue({ kind: 'cancelled' })
    setLanguage('ar')
    render(HistoryExport)
    await fireEvent.click(screen.getByRole('button', { name: t('history.export.save') }))
    await waitFor(() => expect(api.exportHistoryCsv).toHaveBeenCalledOnce())
    const [opts, labels] = vi.mocked(api.exportHistoryCsv).mock.calls[0]
    expect(opts.includeCellNumbers).toBe(false)
    expect(opts.includeIpRange).toBe(false)
    expect(labels.headers.localTime).toBe(t('history.csv.col.localTime'))
    expect(labels.headers.localTime).not.toBe('Local time ({offset})')
  })

  it('shows nothing when the save dialog is cancelled', async () => {
    vi.mocked(api.exportHistoryCsv).mockResolvedValue({ kind: 'cancelled' })
    const onclose = vi.fn()
    render(HistoryExport, { onclose })
    await fireEvent.click(screen.getByRole('button', { name: t('history.export.save') }))
    await waitFor(() => expect(api.exportHistoryCsv).toHaveBeenCalledOnce())
    await Promise.resolve()
    expect(app.toasts).toEqual([])
    expect(screen.queryByRole('alert')).toBeNull()
    expect(onclose).not.toHaveBeenCalled()
  })

  it('shows an error as an alert', async () => {
    vi.mocked(api.exportHistoryCsv).mockRejectedValue('Export stopped: the privacy check found a MAC address.')
    render(HistoryExport)
    await fireEvent.click(screen.getByRole('button', { name: t('history.export.save') }))
    await waitFor(() => expect(screen.getByRole('alert').textContent).toContain('privacy check'))
    expect(app.toasts).toEqual([])
  })
})
