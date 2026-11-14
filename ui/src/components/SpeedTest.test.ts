import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))
vi.mock('../lib/backend', () => ({
  api: {
    runSpeedTest: vi.fn(),
    cancelSpeedTest: vi.fn(),
    speedTestRunning: vi.fn(),
    speedTestHistory: vi.fn(),
    saveSettings: vi.fn(),
    openOoklaTerms: vi.fn(),
  },
  onSpeedTest: vi.fn(),
}))

import { api, onSpeedTest } from '../lib/backend'
import { app } from '../lib/store.svelte'
import { t } from '../lib/i18n'
import { ingestSpeedEvent, resetSpeedTest, speed } from '../lib/speedtest.svelte'
import { resultFixture } from '../lib/speedtest.fixtures'
import { settingsFixture, snap } from '../test-fixtures'
import SpeedTest from './SpeedTest.svelte'

describe('SpeedTest', () => {
  beforeEach(() => {
    resetSpeedTest()
    app.settings = settingsFixture()
    app.snapshot = snap()
    vi.mocked(onSpeedTest).mockResolvedValue(() => {})
    vi.mocked(api.speedTestHistory).mockResolvedValue([])
    vi.mocked(api.speedTestRunning).mockResolvedValue(false)
    vi.mocked(api.runSpeedTest).mockResolvedValue(undefined)
    vi.mocked(api.cancelSpeedTest).mockResolvedValue(undefined)
    vi.mocked(api.saveSettings).mockResolvedValue(undefined)
    vi.mocked(api.openOoklaTerms).mockResolvedValue(undefined)
  })
  afterEach(() => {
    cleanup()
    vi.clearAllMocks()
  })

  it('runs only when pressed and offers cancel while running', async () => {
    render(SpeedTest)
    expect(api.runSpeedTest).not.toHaveBeenCalled()
    await fireEvent.click(screen.getByRole('button', { name: t('speedtest.run') }))
    expect(api.runSpeedTest).toHaveBeenCalledOnce()

    ingestSpeedEvent({ kind: 'started', provider: 'cloudflare', size: 'light', serving: resultFixture().serving })
    ingestSpeedEvent({
      kind: 'progress',
      phase: 'upload',
      fraction: 0.5,
      bytesUsed: 31_000_000,
      live: { idlePingMs: 41, downloadMbps: 108, uploadMbps: 1.3, pingDownMs: 610, pingUpMs: 1450 },
    })
    const cancel = await screen.findByRole('button', { name: t('speedtest.cancel') })
    expect(screen.getByText(/31 MB/)).toBeTruthy()
    const bars = screen.getAllByRole('progressbar')
    expect(bars).toHaveLength(4)
    expect(bars[0].getAttribute('aria-valuenow')).toBe('100')
    expect(bars[2].getAttribute('aria-valuenow')).toBe('50')
    await fireEvent.click(cancel)
    expect(api.cancelSpeedTest).toHaveBeenCalledOnce()
  })

  it('refuses to start during a band change', async () => {
    app.snapshot = snap({ safeApply: { phase: { kind: 'probation' }, secondsLeft: 60, previous: null, requested: null } })
    render(SpeedTest)
    const run = screen.getByRole('button', { name: t('speedtest.run') }) as HTMLButtonElement
    expect(run.disabled).toBe(true)
    expect(screen.getByText(t('speedtest.error.busy'))).toBeTruthy()
  })

  it('shows the diagnosis and the results by band and cell', async () => {
    vi.mocked(api.speedTestHistory).mockResolvedValue([
      resultFixture({ atMs: Date.now() }),
      resultFixture({
        atMs: Date.now() - 1000,
        serving: { ...resultFixture().serving, lteBand: 1, ltePci: 120, nrBand: null },
      }),
    ])
    render(SpeedTest)
    await waitFor(() => expect(screen.getAllByRole('row')).toHaveLength(3))
    expect(screen.getByTestId('diagnosis').textContent).toContain(t('speedtest.diag.upload'))
    expect(screen.getByText('PCI 120 · no 5G')).toBeTruthy()
    expect(screen.getByRole('button', { name: t('speedtest.runAgain') })).toBeTruthy()
    // Loaded latency carries a word, not colour alone.
    expect(screen.getByTestId('stage-busy').parentElement?.textContent).toContain(t('speedtest.lat.high'))
  })

  it('explains ping while busy', async () => {
    render(SpeedTest)
    await fireEvent.click(screen.getByRole('button', { name: t('speedtest.busyHelpLabel') }))
    expect(screen.getByRole('note').textContent).toBe(t('speedtest.busyHelp'))
  })

  it('asks for the Ookla terms before saving Ookla', async () => {
    render(SpeedTest)
    await fireEvent.click(screen.getByLabelText(t('speedtest.settings.ookla')))
    expect(api.saveSettings).not.toHaveBeenCalled()
    expect(screen.getByRole('region', { name: t('speedtest.terms.title') })).toBeTruthy()

    await fireEvent.click(screen.getByRole('button', { name: t('speedtest.terms.eula') }))
    expect(api.openOoklaTerms).toHaveBeenCalledWith('eula')
    await fireEvent.click(screen.getByRole('button', { name: t('speedtest.terms.privacy') }))
    expect(api.openOoklaTerms).toHaveBeenCalledWith('privacy')

    await fireEvent.click(screen.getByRole('button', { name: t('speedtest.terms.accept') }))
    await waitFor(() =>
      expect(api.saveSettings).toHaveBeenCalledWith(
        expect.objectContaining({ speedTest: { provider: 'ookla', size: 'light', ooklaTermsAccepted: true } }),
      ),
    )
    await waitFor(() => expect(screen.queryByRole('region', { name: t('speedtest.terms.title') })).toBeNull())
  })

  it('declining the terms keeps Cloudflare', async () => {
    render(SpeedTest)
    await fireEvent.click(screen.getByLabelText(t('speedtest.settings.ookla')))
    await fireEvent.click(screen.getByRole('button', { name: t('speedtest.terms.decline') }))
    expect(api.saveSettings).not.toHaveBeenCalled()
    expect((screen.getByLabelText(t('speedtest.settings.cloudflare')) as HTMLInputElement).checked).toBe(true)
  })

  it('saves the data size', async () => {
    render(SpeedTest)
    await fireEvent.click(screen.getByLabelText(t('speedtest.settings.full')))
    await waitFor(() =>
      expect(api.saveSettings).toHaveBeenCalledWith(
        expect.objectContaining({ speedTest: expect.objectContaining({ size: 'full' }) }),
      ),
    )
  })

  it('waits during the pause between the two steps of a primary-band apply', () => {
    app.snapshot = snap({
      safeApply: {
        phase: { kind: 'committed' },
        secondsLeft: null,
        previous: null,
        requested: null,
        sequence: { step: 1, total: 2, primary: 3, finalConfig: snap().config!, origin: 'user' },
      },
    })
    render(SpeedTest)
    expect((screen.getByRole('button', { name: t('speedtest.run') }) as HTMLButtonElement).disabled).toBe(true)
    expect(screen.getByText(t('speedtest.error.busy'))).toBeTruthy()
  })

  it('with Ookla the data size is off and the page says how much an Ookla test can use', () => {
    app.settings = settingsFixture({ speedTest: { provider: 'ookla', size: 'light', ooklaTermsAccepted: true } })
    render(SpeedTest)
    expect(screen.getByLabelText(t('speedtest.settings.full')).closest('fieldset')?.disabled).toBe(true)
    expect(screen.getByTestId('ookla-size').textContent).toBe(t('speedtest.settings.ooklaSize'))
  })

  it('with Cloudflare the data size can be chosen and there is no Ookla note', () => {
    render(SpeedTest)
    expect(screen.getByLabelText(t('speedtest.settings.full')).closest('fieldset')?.disabled).toBe(false)
    expect(screen.queryByTestId('ookla-size')).toBeNull()
  })

  it("the Ookla terms say the test isn't limited by the data size", async () => {
    render(SpeedTest)
    await fireEvent.click(screen.getByLabelText(t('speedtest.settings.ookla')))
    const terms = screen.getByRole('region', { name: t('speedtest.terms.title') })
    expect(terms.textContent).toContain(t('speedtest.terms.dataUse'))
    expect(screen.getByTestId('ookla-size')).toBeTruthy()
  })

  it('shows a failed run as a plain reason with its cause underneath', async () => {
    render(SpeedTest)
    const cause = 'Ookla Speedtest stopped without a result: during the upload test: Cannot read from socket (exit 2)'
    ingestSpeedEvent({ kind: 'failed', message: cause, reason: 'ookla' })
    await waitFor(() => {
      const alert = screen.getByRole('alert')
      expect(alert.textContent).toContain(t('speedtest.fail.ookla'))
      expect(alert.textContent).toContain(t('speedtest.failCause', { message: cause }))
    })
  })

  it('explains a timeout in plain words', async () => {
    render(SpeedTest)
    ingestSpeedEvent({ kind: 'failed', message: 'error sending request: operation timed out', reason: 'timeout' })
    await waitFor(() => expect(screen.getByRole('alert').textContent).toContain(t('speedtest.fail.timeout')))
  })

  it('marks a result whose band changed during the test as not comparable', async () => {
    vi.mocked(api.speedTestHistory).mockResolvedValue([
      { ...resultFixture({ atMs: Date.now() }), invalidReason: 'bandChanged' } as never,
      resultFixture({ atMs: Date.now() - 1000 }),
    ])
    render(SpeedTest)
    await waitFor(() => expect(screen.getAllByRole('row')).toHaveLength(3))
    const rows = screen.getAllByTestId('not-comparable')
    expect(rows).toHaveLength(1)
    expect(rows[0].textContent).toBe(t('speedtest.history.notComparable'))
    expect(screen.getByTestId('last-not-comparable').textContent?.trim()).toBe(t('speedtest.history.notComparable'))
  })

  it('locks the settings while a test runs', async () => {
    speed.running = true
    render(SpeedTest)
    expect(screen.getByLabelText(t('speedtest.settings.full')).closest('fieldset')?.disabled).toBe(true)
  })

  it('puts the start button above the result, and shows 10 results a page', async () => {
    const list = Array.from({ length: 23 }, (_, i) => resultFixture({ atMs: 1000 + i }))
    vi.mocked(api.speedTestHistory).mockResolvedValue(list)
    const view = render(SpeedTest)
    const rows = () => view.container.querySelectorAll('tbody tr').length
    await waitFor(() => expect(rows()).toBe(10))

    const run = screen.getByRole('button', { name: t('speedtest.runAgain') })
    const result = screen.getByTestId('stage-download')
    expect(run.compareDocumentPosition(result) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()

    expect(screen.getByText(t('speedtest.history.page', { n: '1', total: '3' }))).toBeTruthy()
    expect((screen.getByRole('button', { name: t('speedtest.history.newer') }) as HTMLButtonElement).disabled).toBe(true)
    await fireEvent.click(screen.getByRole('button', { name: t('speedtest.history.older') }))
    await fireEvent.click(screen.getByRole('button', { name: t('speedtest.history.older') }))
    expect(rows()).toBe(3)
    expect((screen.getByRole('button', { name: t('speedtest.history.older') }) as HTMLButtonElement).disabled).toBe(true)
  })

  it('leaves out the "Started by" column, which only History shows', async () => {
    vi.mocked(api.speedTestHistory).mockResolvedValue([resultFixture({ source: 'auto' })])
    const view = render(SpeedTest)
    await waitFor(() => expect(view.container.querySelectorAll('tbody tr').length).toBe(1))
    expect(screen.queryByRole('columnheader', { name: t('speedtest.source.label') })).toBeNull()
    expect(screen.queryByTestId('source')).toBeNull()
  })

  it('shows no page buttons for one page', async () => {
    vi.mocked(api.speedTestHistory).mockResolvedValue([resultFixture()])
    const view = render(SpeedTest)
    await waitFor(() => expect(view.container.querySelectorAll('tbody tr').length).toBe(1))
    expect(screen.queryByRole('navigation', { name: t('speedtest.history.pages') })).toBeNull()
  })
})
