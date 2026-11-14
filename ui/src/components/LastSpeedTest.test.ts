import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))
vi.mock('../lib/backend', () => ({
  api: { speedTestRunning: vi.fn(), speedTestHistory: vi.fn(), runSpeedTest: vi.fn(), cancelSpeedTest: vi.fn() },
  onSpeedTest: vi.fn(),
  onScan: vi.fn(),
}))

import { api, onSpeedTest } from '../lib/backend'
import { app } from '../lib/store.svelte'
import { t } from '../lib/i18n'
import { scan } from '../lib/scan.svelte'
import { diagnose, ingestSpeedEvent, resetSpeedTest, speed } from '../lib/speedtest.svelte'
import { resultFixture } from '../lib/speedtest.fixtures'
import { settingsFixture, snap } from '../test-fixtures'
import LastSpeedTest from './LastSpeedTest.svelte'

const serving = resultFixture().serving

describe('LastSpeedTest', () => {
  beforeEach(() => {
    resetSpeedTest()
    app.settings = settingsFixture()
    app.snapshot = snap()
    app.tab = 'dashboard'
    scan.file = null
    vi.mocked(onSpeedTest).mockResolvedValue(() => {})
    vi.mocked(api.speedTestRunning).mockResolvedValue(false)
    vi.mocked(api.speedTestHistory).mockResolvedValue([])
    vi.mocked(api.runSpeedTest).mockResolvedValue(undefined)
    vi.mocked(api.cancelSpeedTest).mockResolvedValue(undefined)
  })
  afterEach(() => {
    cleanup()
    vi.clearAllMocks()
    scan.file = null
    app.snapshot = null
  })

  it('shows the last result with its band and cell, and what limits it', async () => {
    const r = resultFixture({ atMs: Date.now() })
    vi.mocked(api.speedTestHistory).mockResolvedValue([r])
    render(LastSpeedTest)
    await waitFor(() => expect(screen.getByText(t('speedtest.last.title'))).toBeTruthy())
    expect(screen.getByText(/B3 \+ n41.*PCI 103/)).toBeTruthy()
    expect(screen.getByTestId('home-download').textContent).toContain('108')
    expect(screen.getByTestId('home-upload').textContent).toContain('1.3')
    expect(screen.getByTestId('home-ping').textContent).toContain('41')
    expect(screen.getByText(diagnose(r).headline)).toBeTruthy()
  })

  it('runs the test in place from "Run again"', async () => {
    vi.mocked(api.speedTestHistory).mockResolvedValue([resultFixture()])
    render(LastSpeedTest)
    const again = await screen.findByRole('button', { name: t('speedtest.runAgain') })
    await fireEvent.click(again)
    expect(api.runSpeedTest).toHaveBeenCalledTimes(1)
    expect(app.tab).toBe('dashboard')
  })

  it('keeps Run disabled from the click until the test is seen running, and frees it on a refusal', async () => {
    render(LastSpeedTest)
    const run = screen.getByRole('button', { name: t('speedtest.last.run') }) as HTMLButtonElement
    await fireEvent.click(run)
    await waitFor(() => expect(api.runSpeedTest).toHaveBeenCalledOnce())
    // The command has returned, but 'started' hasn't arrived yet.
    expect(run.disabled).toBe(true)
    ingestSpeedEvent({ kind: 'started', provider: 'cloudflare', size: 'light', serving })
    await waitFor(() => expect(screen.getByRole('button', { name: t('speedtest.cancel') })).toBeTruthy())
    ingestSpeedEvent({ kind: 'cancelled' })
    const again = (await screen.findByRole('button', { name: t('speedtest.last.run') })) as HTMLButtonElement
    expect(again.disabled).toBe(false)

    vi.mocked(api.runSpeedTest).mockRejectedValueOnce('a speed test is already running')
    await fireEvent.click(again)
    await waitFor(() => expect(screen.getByText(t('speedtest.error.running'))).toBeTruthy())
    expect(again.disabled).toBe(false)
  })

  it('leads to the Speed test page from "More details"', async () => {
    render(LastSpeedTest)
    await fireEvent.click(screen.getByRole('button', { name: t('speedtest.home.moreDetails') }))
    expect(app.tab).toBe('speedtest')
  })

  it('invites a first test when there is none, and starts it', async () => {
    render(LastSpeedTest)
    expect(screen.getByText(t('speedtest.last.none'))).toBeTruthy()
    await fireEvent.click(screen.getByRole('button', { name: t('speedtest.last.run') }))
    expect(api.runSpeedTest).toHaveBeenCalledTimes(1)
    expect(app.tab).toBe('dashboard')
  })

  it('shows progress, the stage and live numbers while running, with Cancel', async () => {
    render(LastSpeedTest)
    ingestSpeedEvent({ kind: 'started', provider: 'cloudflare', size: 'light', serving })
    ingestSpeedEvent({
      kind: 'progress',
      phase: 'download',
      fraction: 0.5,
      bytesUsed: 1_000_000,
      live: { idlePingMs: 40, downloadMbps: 77, uploadMbps: null, pingDownMs: null, pingUpMs: null },
    })
    const bar = await screen.findByRole('progressbar', { name: t('speedtest.home.progressLabel') })
    expect(bar.getAttribute('aria-valuenow')).toBe('33')
    expect(screen.getByText(t('speedtest.testingNow'))).toBeTruthy()
    expect(screen.getByRole('status').textContent).toBe(t('speedtest.home.stage.download'))
    expect(screen.getByTestId('home-download').textContent).toContain('77')
    expect(screen.getByText(t('speedtest.home.using', { provider: t('speedtest.provider.cloudflare') }))).toBeTruthy()
    await fireEvent.click(screen.getByRole('button', { name: t('speedtest.cancel') }))
    expect(api.cancelSpeedTest).toHaveBeenCalledTimes(1)
  })

  it('shows an indeterminate bar after a reload mid-test', async () => {
    vi.mocked(api.speedTestRunning).mockResolvedValue(true)
    render(LastSpeedTest)
    const bar = await screen.findByRole('progressbar')
    expect(bar.getAttribute('aria-valuenow')).toBeNull()
    expect(bar.classList.contains('opaque')).toBe(true)
    expect(screen.getByRole('status').textContent).toBe(t('speedtest.home.stage.starting'))
    expect(screen.getByRole('button', { name: t('speedtest.cancel') })).toBeTruthy()
  })

  it("doesn't offer Cancel for an auto-scan's own test", async () => {
    scan.file = { status: 'running' } as never
    render(LastSpeedTest)
    ingestSpeedEvent({ kind: 'started', provider: 'cloudflare', size: 'light', serving })
    await screen.findByRole('progressbar')
    expect(screen.queryByRole('button', { name: t('speedtest.cancel') })).toBeNull()
    expect(screen.getByText(t('speedtest.home.byScan'))).toBeTruthy()
    await fireEvent.click(screen.getByRole('button', { name: t('speedtest.home.openScan') }))
    expect(app.tab).toBe('scan')
  })

  it('says why it cannot run during a band change', async () => {
    app.snapshot = snap({ safeApply: { phase: { kind: 'probation' }, secondsLeft: 9, previous: null, requested: null } as never })
    render(LastSpeedTest)
    const run = screen.getByRole('button', { name: t('speedtest.last.run') }) as HTMLButtonElement
    expect(run.disabled).toBe(true)
    const note = screen.getByText(t('speedtest.error.busy'))
    expect(run.getAttribute('aria-describedby')).toBe(note.id)
  })

  it('shows a failure until a later test finishes', async () => {
    vi.mocked(api.speedTestHistory).mockResolvedValue([resultFixture({ atMs: 1_000 })])
    render(LastSpeedTest)
    await screen.findByText(t('speedtest.last.title'))
    ingestSpeedEvent({ kind: 'started', provider: 'cloudflare', size: 'light', serving })
    ingestSpeedEvent({ kind: 'failed', message: 'timed out', reason: 'timeout' })
    const alert = await screen.findByRole('alert')
    expect(alert.textContent).toContain(t('speedtest.home.failed'))
    expect(alert.textContent).toContain(t('speedtest.fail.timeout'))
    expect(screen.getByRole('button', { name: t('speedtest.home.tryAgain') })).toBeTruthy()
    expect(screen.getByText(/^Previous result:/)).toBeTruthy()

    ingestSpeedEvent({ kind: 'finished', ...resultFixture({ atMs: Date.now() + 1 }) })
    await waitFor(() => expect(screen.queryByRole('alert')).toBeNull())
    expect(screen.getByRole('button', { name: t('speedtest.runAgain') })).toBeTruthy()
  })

  it('hides an error older than the last result', async () => {
    vi.mocked(api.speedTestHistory).mockResolvedValue([resultFixture({ atMs: 5_000 })])
    render(LastSpeedTest)
    await screen.findByText(t('speedtest.last.title'))
    speed.error = 'old failure'
    speed.errorAtMs = 4_000
    await Promise.resolve()
    expect(screen.queryByRole('alert')).toBeNull()
    expect(screen.queryByText('old failure')).toBeNull()
  })
})
