import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))
vi.mock('./backend', () => ({
  api: {
    runSpeedTest: vi.fn(),
    cancelSpeedTest: vi.fn(),
    speedTestRunning: vi.fn(),
    speedTestHistory: vi.fn(),
    saveSettings: vi.fn(),
  },
  onSpeedTest: vi.fn(),
}))

import { api, onSpeedTest } from './backend'
import { t } from './i18n'
import { app, updateSettings } from './store.svelte'
import { settingsFixture } from '../test-fixtures'
import { resultFixture } from './speedtest.fixtures'
import {
  busyPing,
  diagnose,
  duration,
  ensureSpeedTest,
  ingestSpeedEvent,
  invalidReason,
  latLevel,
  overallFraction,
  resetSpeedTest,
  saveSpeedSettings,
  servingBands,
  servingDetail,
  speed,
  stageKey,
  startTest,
  whenLabel,
} from './speedtest.svelte'

describe('speed-test helpers', () => {
  it('names the bottleneck: slow upload with lag while uploading', () => {
    const d = diagnose(resultFixture())
    expect(d.headline).toBe(t('speedtest.diag.upload'))
    expect(d.detail).toBe(t('speedtest.diag.uploadLag', { time: '1.5 s' }))
  })

  it('names lag under load when upload is fast', () => {
    const d = diagnose(resultFixture({ uploadMbps: 20, pingUpMs: 90, pingDownMs: 700 }))
    expect(d.headline).toBe(t('speedtest.diag.lag'))
    expect(d.detail).toContain('700')
  })

  it('names slow download, a good link, a partial run and no result', () => {
    expect(diagnose(resultFixture({ downloadMbps: 4, uploadMbps: 10, pingDownMs: 80, pingUpMs: 90 })).headline).toBe(
      t('speedtest.diag.download'),
    )
    expect(diagnose(resultFixture({ uploadMbps: 30, pingDownMs: 60, pingUpMs: 80 })).headline).toBe(
      t('speedtest.diag.good'),
    )
    expect(diagnose(resultFixture({ uploadMbps: null })).headline).toBe(t('speedtest.diag.partial'))
    expect(diagnose(null).headline).toBe(t('speedtest.diag.none'))
  })

  it('picks the worse loaded ping and grades it', () => {
    expect(busyPing({ pingDownMs: 610, pingUpMs: 1450 })).toEqual({ ms: 1450, during: 'upload' })
    expect(busyPing({ pingDownMs: 610, pingUpMs: null })).toEqual({ ms: 610, during: 'download' })
    expect(busyPing({ pingDownMs: null, pingUpMs: null })).toEqual({ ms: null, during: null })
    expect(latLevel(50)).toBe('low')
    expect(latLevel(150)).toBe('moderate')
    expect(latLevel(1450)).toBe('high')
    expect(duration(610)).toBe('610 ms')
  })

  it('labels the serving cell by band and PCI', () => {
    const s = resultFixture().serving
    expect(servingBands(s)).toBe('B3 + n41')
    expect(servingDetail(s)).toBe('PCI 103')
    const lteOnly = { ...s, lteBand: 1, ltePci: 120, nrBand: null }
    expect(servingDetail(lteOnly)).toBe('PCI 120 · no 5G')
  })

  it('says today and yesterday', () => {
    const now = new Date(2026, 9, 30, 15, 0).getTime()
    expect(whenLabel(new Date(2026, 9, 30, 14, 2).getTime(), now)).toBe('Today 14:02')
    expect(whenLabel(new Date(2026, 9, 29, 21, 10).getTime(), now)).toBe('Yesterday 21:10')
  })
})

describe('overall progress', () => {
  it('has no figure while it cannot be known', () => {
    expect(overallFraction('installing', 0.5)).toBeNull()
    expect(overallFraction('external', 0.5)).toBeNull()
    expect(overallFraction(null, 0)).toBeNull()
  })

  it('weights ping, download and upload', () => {
    expect(overallFraction('idlePing', 0)).toBe(0)
    expect(overallFraction('idlePing', 1)).toBeCloseTo(0.1)
    expect(overallFraction('download', 0)).toBeCloseTo(0.1)
    expect(overallFraction('download', 0.5)).toBeCloseTo(0.325)
    expect(overallFraction('upload', 0)).toBeCloseTo(0.55)
    expect(overallFraction('upload', 1)).toBeCloseTo(1)
    expect(overallFraction('done', 0)).toBe(1)
  })

  it('clamps the stage fraction', () => {
    expect(overallFraction('download', -1)).toBeCloseTo(0.1)
    expect(overallFraction('download', 2)).toBeCloseTo(0.55)
    expect(overallFraction('idlePing', Number.NaN)).toBe(0)
  })

  it('names each stage', () => {
    expect(stageKey(null)).toBe('speedtest.home.stage.starting')
    expect(stageKey('idlePing')).toBe('speedtest.home.stage.idlePing')
    expect(stageKey('download')).toBe('speedtest.home.stage.download')
    expect(stageKey('upload')).toBe('speedtest.home.stage.upload')
    expect(stageKey('installing')).toBe('speedtest.installing')
    expect(stageKey('external')).toBe('speedtest.external')
    expect(stageKey('done')).toBe('speedtest.home.stage.saving')
  })
})

describe('speed-test state', () => {
  beforeEach(() => {
    resetSpeedTest()
    app.settings = settingsFixture()
  })
  afterEach(() => vi.clearAllMocks())

  it('follows a run from start to result', () => {
    const serving = resultFixture().serving
    ingestSpeedEvent({ kind: 'started', provider: 'cloudflare', size: 'light', serving })
    expect(speed.running).toBe(true)
    ingestSpeedEvent({
      kind: 'progress',
      phase: 'download',
      fraction: 0.5,
      bytesUsed: 10_000_000,
      live: { idlePingMs: 41, downloadMbps: 90, uploadMbps: null, pingDownMs: 500, pingUpMs: null },
    })
    expect(speed.phase).toBe('download')
    expect(speed.live.downloadMbps).toBe(90)
    ingestSpeedEvent({ kind: 'finished', ...resultFixture({ atMs: 5 }) })
    expect(speed.running).toBe(false)
    expect(speed.history[0].atMs).toBe(5)
    expect('kind' in speed.history[0]).toBe(false)
  })

  it('records a cancel and a failure without a result', () => {
    ingestSpeedEvent({ kind: 'cancelled' })
    expect(speed.cancelled).toBe(true)
    ingestSpeedEvent({ kind: 'failed', message: 'dns error: No such host is known.', reason: 'noInternet' })
    expect(speed.error).toBe(t('speedtest.fail.noInternet'))
    expect(speed.errorCause).toContain('No such host')
    expect(speed.history).toHaveLength(0)
  })

  it('clears a failure when a later test finishes', () => {
    ingestSpeedEvent({ kind: 'failed', message: 'timed out', reason: 'timeout' })
    expect(speed.error).toBe(t('speedtest.fail.timeout'))
    expect(speed.errorAtMs).not.toBeNull()
    ingestSpeedEvent({ kind: 'finished', ...resultFixture({ atMs: 9 }) })
    expect(speed.error).toBeNull()
    expect(speed.errorCause).toBeNull()
    expect(speed.errorAtMs).toBeNull()
  })

  it('shows the band-change refusal in the user language', async () => {
    vi.mocked(api.runSpeedTest).mockRejectedValue('Finish or revert the current band change first.')
    await startTest()
    expect(speed.error).toBe(t('speedtest.error.busy'))
    expect(speed.errorAtMs).not.toBeNull()
  })

  it('loads the history once and listens for events', async () => {
    vi.mocked(onSpeedTest).mockResolvedValue(() => {})
    vi.mocked(api.speedTestHistory).mockResolvedValue([resultFixture({ atMs: 2 }), resultFixture({ atMs: 1 })])
    vi.mocked(api.speedTestRunning).mockResolvedValue(false)
    await Promise.all([ensureSpeedTest(), ensureSpeedTest()])
    expect(onSpeedTest).toHaveBeenCalledOnce()
    expect(speed.history.map((r) => r.atMs)).toEqual([2, 1])
  })

  it('saves the speed-test settings through save_settings', async () => {
    vi.mocked(api.saveSettings).mockResolvedValue(undefined)
    await saveSpeedSettings({ size: 'full' })
    expect(api.saveSettings).toHaveBeenCalledWith(
      expect.objectContaining({ speedTest: { provider: 'cloudflare', size: 'full', ooklaTermsAccepted: false } }),
    )
    expect(app.settings?.speedTest.size).toBe('full')
  })
})

describe('speed-test settings and markers', () => {
  beforeEach(() => {
    app.settings = settingsFixture()
  })
  afterEach(() => vi.clearAllMocks())

  it('a speed-test settings save waits for another settings save and keeps it', async () => {
    let release!: () => void
    vi.mocked(api.saveSettings)
      .mockImplementationOnce(() => new Promise<void>((r) => (release = r)))
      .mockResolvedValue(undefined)
    const star = updateSettings((s) => ({ ...s, automation: { ...s.automation, primaryBand: 3 } }))
    const size = saveSpeedSettings({ size: 'full' })
    await new Promise((r) => setTimeout(r, 0))
    expect(api.saveSettings).toHaveBeenCalledOnce()
    release()
    await Promise.all([star, size])
    expect(app.settings?.automation.primaryBand).toBe(3)
    expect(app.settings?.speedTest.size).toBe('full')
  })

  it('reads the not-comparable marker when there is one', () => {
    expect(invalidReason(resultFixture())).toBeNull()
    expect(invalidReason({ ...resultFixture(), invalidReason: 'bandChanged' } as never)).toBe('bandChanged')
    expect(invalidReason({ ...resultFixture(), invalidReason: null } as never)).toBeNull()
  })
})

describe('a test that lost its upload', () => {
  it('is named as incomplete, not as a changed band, and is told apart from the others', async () => {
    const { incomplete, notComparableKey } = await import('./speedtest.svelte')
    const { resultFixture } = await import('./speedtest.fixtures')
    const cut = resultFixture({ uploadMbps: null, invalidReason: 'incomplete' })
    expect(notComparableKey(cut)).toBe('speedtest.history.incomplete')
    expect(incomplete(cut)).toBe(true)
    expect(incomplete(resultFixture({ invalidReason: 'bandChange' }))).toBe(false)
    expect(incomplete(resultFixture())).toBe(false)
  })
})
