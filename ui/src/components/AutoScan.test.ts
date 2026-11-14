import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))
vi.mock('../lib/backend', () => ({
  api: {
    startScan: vi.fn(),
    stopScan: vi.fn(),
    continueScan: vi.fn(),
    dismissScan: vi.fn(),
    applyConfig: vi.fn(),
    speedTestHistory: vi.fn(),
    speedTestRunning: vi.fn(),
  },
  onScan: vi.fn(),
  onSpeedTest: vi.fn(),
}))

import { api } from '../lib/backend'
import { app } from '../lib/store.svelte'
import { history, resetHistory } from '../lib/history.svelte'
import { resetScan, scan } from '../lib/scan.svelte'
import { speed } from '../lib/speedtest.svelte'
import { resultFixture } from '../lib/speedtest.fixtures'
import { t } from '../lib/i18n'
import { settingsFixture } from '../test-fixtures'
import AutoScan from './AutoScan.svelte'
import type { ScanFile } from '../lib/bindings/ScanFile'
import type { ScanResult } from '../lib/bindings/ScanResult'

const B1 = '1'
const B3 = '4'
const B1B3 = '5'
const start = { networkMode: { kind: 'auto' }, lteBands: B1B3, legacyBandRaw: '0', nrOption: 'nsa' } as ScanFile['start']

function result(bands: string, o: Partial<ScanResult> = {}): ScanResult {
  return {
    atMs: 0, bands, isStart: false, outcome: 'measured', note: null, lteBand: 3, ltePci: 103, nrBand: 41, got5g: true,
    lteRsrp: null, lteSinr: null, nrSinr: null, downloadMbps: 50, uploadMbps: 5, idlePingMs: 40, pingDownMs: 200,
    pingUpMs: 300, bytesUsed: 0, tests: 1, downloadMinMbps: 50, downloadMaxMbps: 50, uploadMinMbps: 5, uploadMaxMbps: 5, ...o,
  }
}
function file(o: Partial<ScanFile> = {}): ScanFile {
  return {
    version: 1, host: 'h', driver: 'mock', mode: 'quick', status: 'done', step: 'testing', start, queue: [B1B3, B1, B3],
    next: 3, combosAdded: false, results: [], startedMs: 0, returnFailed: false, repeats: 1, test: 1, model: null, addOnBands: [], customSets: [], ...o,
  }
}

describe('AutoScan', () => {
  beforeEach(() => {
    resetScan()
    app.settings = settingsFixture()
    for (const f of [api.startScan, api.stopScan, api.continueScan, api.dismissScan, api.applyConfig])
      vi.mocked(f).mockResolvedValue(undefined)
  })
  afterEach(() => {
    cleanup()
    vi.clearAllMocks()
  })

  it('shows time, data and the warning first, and unticks a band with no service', async () => {
    render(AutoScan, { supported: [1, 3, 43], tones: new Map([[43, 'noService' as const]]) })
    // Only the number is isolated, so Arabic reads it before its unit. The first sentence stands alone.
    const warning = t('scan.warning', { grace: t('automation.seconds', { n: '\u206660\u2069' }) })
    const firstStop = warning.indexOf('. ') + 1
    const note = screen.getByRole('note').textContent
    expect(note).toContain(warning.slice(0, firstStop))
    expect(note).toContain(warning.slice(firstStop).trim())
    expect(screen.getByTestId('scan-estimate').textContent).toContain('240')
    expect((screen.getByLabelText(/^B43/) as HTMLInputElement).checked).toBe(false)
    // The past result is an icon with words, not a dashed border alone.
    expect(screen.getByLabelText(/^B43/).closest('label')?.textContent).toContain(t('bands.legend.noService'))

    await fireEvent.click(screen.getByRole('button', { name: t('scan.start') }))
    expect(api.startScan).toHaveBeenCalledWith('quick', [1, 3], 2, [])
  })

  it('needs a band, and Thorough says it is a most-case figure', async () => {
    render(AutoScan, { supported: [1], tones: new Map() })
    await fireEvent.click(screen.getByLabelText(t('scan.modes.thorough')))
    expect(screen.getByTestId('scan-estimate').textContent).toContain(t('scan.upTo', { n: '⁦2⁩' }))
    await fireEvent.click(screen.getByLabelText('B1'))
    expect((screen.getByRole('button', { name: t('scan.start') }) as HTMLButtonElement).disabled).toBe(true)
  })

  it('offers Stop while running, and a way out of a return that never ends', async () => {
    scan.file = file({ status: 'running', next: 1, step: 'settling', results: [result(B1B3, { isStart: true })] })
    const view = render(AutoScan, { supported: [1, 3], tones: new Map() })
    await fireEvent.click(screen.getByRole('button', { name: t('scan.stop') }))
    expect(api.stopScan).toHaveBeenCalledOnce()
    expect(view.container.textContent).toContain(t('scan.step.settling'))
    cleanup()

    scan.file = file({ status: 'returning', next: 3 })
    render(AutoScan, { supported: [1, 3], tones: new Map() })
    await fireEvent.click(screen.getByRole('button', { name: t('scan.stopReturn') }))
    expect(api.stopScan).toHaveBeenCalledTimes(2)
    expect(screen.getByTestId('scan-running').textContent).toContain(t('scan.step.returning'))
  })

  it('ranks by the chosen priority and marks 5G', async () => {
    scan.file = file({
      results: [
        result(B1, { downloadMbps: 200, uploadMbps: 1, pingDownMs: 900, pingUpMs: 1500, got5g: false, nrBand: null }),
        result(B3, { downloadMbps: 40, uploadMbps: 30, pingDownMs: 100, pingUpMs: 150 }),
      ],
    })
    render(AutoScan, { supported: [1, 3, 7], tones: new Map() })
    const first = () => screen.getAllByTestId('scan-row')[0].textContent
    expect(first()).toContain('B3')
    expect(first()).toContain('n41')
    await fireEvent.click(screen.getByLabelText(t('scan.priority.download')))
    await waitFor(() => expect(first()).toContain('B1'))
    expect(first()).toContain(t('scan.no5g'))
  })

  it('Apply best goes through the normal apply with the start config otherwise unchanged', async () => {
    scan.file = file({ results: [result(B3, { downloadMbps: 90 }), result(B1B3, { isStart: true, downloadMbps: 10, uploadMbps: 1 })] })
    render(AutoScan, { supported: [1, 3, 7], tones: new Map() })
    const apply = screen.getByRole('button', { name: new RegExp(t('scan.applyBest', { bands: 'X' }).replace('X', '.*')) })
    await fireEvent.click(apply)
    await waitFor(() => expect(api.applyConfig).toHaveBeenCalledWith({ ...start, lteBands: B3 }))
    await waitFor(() => expect(api.dismissScan).toHaveBeenCalledOnce())
  })

  it('offers no Apply best when the current bands already won', () => {
    scan.file = file({ results: [result(B1B3, { isStart: true, downloadMbps: 90 }), result(B3, { downloadMbps: 10, uploadMbps: 1 })] })
    render(AutoScan, { supported: [1, 3, 7], tones: new Map() })
    expect(screen.getByTestId('scan-summary').textContent).toContain(t('scan.summary.keep'))
    expect(screen.queryByRole('button', { name: /B3/ })).toBeNull()
  })

  it('explains an interrupted scan and can continue it', async () => {
    scan.file = file({ status: 'interrupted', next: 1, results: [result(B1B3, { isStart: true })] })
    render(AutoScan, { supported: [1, 3], tones: new Map() })
    expect(screen.getByText(t('scan.interrupted'))).toBeTruthy()
    await fireEvent.click(screen.getByRole('button', { name: t('scan.continue') }))
    expect(api.continueScan).toHaveBeenCalledOnce()
  })

  it('runs 2 tests per config by default and 3 for Thorough, until the user picks', async () => {
    render(AutoScan, { supported: [1], tones: new Map() })
    const on = (n: string) => (screen.getByLabelText(n) as HTMLInputElement).checked
    expect(on('2')).toBe(true)
    await fireEvent.click(screen.getByLabelText(t('scan.modes.thorough')))
    expect(on('3')).toBe(true)
    await fireEvent.click(screen.getByLabelText('1'))
    await fireEvent.click(screen.getByLabelText(t('scan.modes.quick')))
    expect(on('1')).toBe(true)
    await fireEvent.click(screen.getByRole('button', { name: t('scan.start') }))
    expect(api.startScan).toHaveBeenCalledWith('quick', [1], 1, [])
  })

  it('suggests another time before settling, and gives the range when it swings', () => {
    scan.file = file({
      queue: [B3],
      results: [result(B3, { tests: 2, downloadMbps: 75, downloadMinMbps: 30, downloadMaxMbps: 120, uploadMbps: 3 })],
    })
    render(AutoScan, { supported: [1, 3], tones: new Map() })
    const note = screen.getByTestId('scan-time-of-day').textContent ?? ''
    expect(note).toContain(t('scan.summary.timeOfDay'))
    expect(note).toContain(t('scan.summary.swing', { low: '⁦30.0⁩', high: '⁦120⁩' }))
  })

  it('gives no time-of-day advice without a recommendation', () => {
    scan.file = file({ results: [result(B1, { outcome: 'testFailed', downloadMbps: null, uploadMbps: null })] })
    render(AutoScan, { supported: [1, 3], tones: new Map() })
    expect(screen.queryByTestId('scan-time-of-day')).toBeNull()
  })

  it('runs 3 tests per config when asked, and counts them in the estimate', async () => {
    render(AutoScan, { supported: [1], tones: new Map() })
    const before = screen.getByTestId('scan-estimate').textContent
    await fireEvent.click(screen.getByLabelText('3'))
    expect(screen.getByTestId('scan-estimate').textContent).not.toBe(before)
    expect(screen.getByTestId('scan-estimate').textContent).toContain('240')
    await fireEvent.click(screen.getByRole('button', { name: t('scan.start') }))
    expect(api.startScan).toHaveBeenCalledWith('quick', [1], 3, [])
  })

  it('says which test is running, and lists what has finished and what comes next', () => {
    scan.file = file({
      status: 'running', next: 1, step: 'testing', repeats: 3, test: 2,
      results: [result(B1B3, { tests: 3, downloadMbps: 100, downloadMinMbps: 80, downloadMaxMbps: 120 })],
    })
    render(AutoScan, { supported: [1, 3], tones: new Map() })
    const shown = screen.getByTestId('scan-running').textContent
    expect(shown).toContain(t('scan.testOf', { n: '⁦2⁩', total: '⁦3⁩' }))
    expect(screen.getByTestId('scan-timeline').textContent).toContain('100')
    expect(shown).toContain(t('scan.stopHint'))
    expect(screen.getByTestId('scan-up-next').textContent).toContain('B3')
  })

  it('after Stop offers Resume and Restart, keeps the results, and Restart reuses the setup', async () => {
    scan.file = file({
      status: 'stopped', mode: 'thorough', repeats: 2, next: 2, queue: [B1B3, B1, B3],
      results: [result(B1B3, { isStart: true })],
    })
    render(AutoScan, { supported: [1, 3], tones: new Map() })
    expect(screen.getAllByTestId('scan-row')).toHaveLength(1)
    await fireEvent.click(screen.getByRole('button', { name: t('scan.continue') }))
    expect(api.continueScan).toHaveBeenCalledOnce()
    await fireEvent.click(screen.getByRole('button', { name: t('scan.restart') }))
    await waitFor(() => expect(api.startScan).toHaveBeenCalledWith('thorough', [1, 3], 2, []))
  })

  it('a finished scan can run again', async () => {
    scan.file = file({ results: [result(B1B3, { isStart: true })] })
    render(AutoScan, { supported: [1, 3], tones: new Map() })
    expect(screen.queryByRole('button', { name: t('scan.restart') })).toBeNull()
    await fireEvent.click(screen.getByRole('button', { name: t('scan.runAgain') }))
    expect(api.startScan).toHaveBeenCalledOnce()
  })

  it('sums up in plain words what was best, what else worked and what has no service', () => {
    const B20 = (1 << 19).toString(16)
    const B7 = (1 << 6).toString(16)
    scan.file = file({
      queue: [B1B3, B3, B20, B7],
      next: 4,
      results: [
        result(B1B3, { isStart: true, downloadMbps: 95, uploadMbps: 1.7, pingDownMs: 400, pingUpMs: 500 }),
        result(B3, { downloadMbps: 116, uploadMbps: 2.8, pingDownMs: 200, pingUpMs: 300 }),
        result(B20, { downloadMbps: 7.2, uploadMbps: 2.3, got5g: false, nrBand: null, lteBand: 20, ltePci: 120 }),
        result(B7, { outcome: 'noService', downloadMbps: null, uploadMbps: null, got5g: false, nrBand: null, lteBand: null }),
      ],
    })
    render(AutoScan, { supported: [1, 3, 7, 20], tones: new Map() })
    const text = screen.getByTestId('scan-summary').textContent ?? ''
    expect(text).toContain(t('scan.summary.alone', { band: '⁦B3⁩' }))
    expect(text).toContain('116')
    expect(text).toContain('n41')
    expect(text).toContain(t('scan.summary.lose5g', { list: t('scan.summary.alone', { band: '⁦B20⁩' }) }))
    const dead = screen.getByTestId('scan-no-service') as HTMLDetailsElement
    expect(dead.open).toBe(false)
    expect(dead.textContent).toContain('B7')
    expect(screen.getAllByTestId('scan-row').some((r) => r.textContent?.includes('B7'))).toBe(false)
    expect(text).toContain(t('scan.summary.triedSome', { n: '⁦3⁩', total: '⁦4⁩' }))
    expect(text).toContain(t('scan.summary.noCombos2'))
    expect(text).toContain(t('scan.summary.testedOnce'))
  })

  it('says there is not enough data instead of recommending the only band set with a number', () => {
    scan.file = file({
      status: 'stopped',
      next: 2,
      results: [
        result(B1B3, { isStart: true, outcome: 'testFailed', downloadMbps: null, uploadMbps: null, got5g: true }),
        result(B1, { downloadMbps: 19.2, uploadMbps: 1.1, got5g: false, nrBand: null }),
      ],
    })
    render(AutoScan, { supported: [1, 3], tones: new Map() })
    expect(screen.getByTestId('scan-summary').textContent).toContain(t('scan.summary.notEnough'))
    expect(screen.getByTestId('scan-summary').textContent).not.toContain(t('scan.summary.switch', { bands: '' }).trim())
    expect(screen.queryByRole('button', { name: new RegExp(t('scan.applyBest', { bands: 'X' }).replace('X', '.*')) })).toBeNull()
  })

  it('says why a measured band set has no 5G when the router explains it', () => {
    scan.file = file({
      results: [
        result(B1, { got5g: false, nrBand: null, endc: false, endcRestricted: false }),
        result(B3, { got5g: true, endc: true, endcRestricted: false }),
      ],
    })
    render(AutoScan, { supported: [1, 3], tones: new Map() })
    const reasons = screen.getAllByTestId('no-5g-reason')
    expect(reasons).toHaveLength(1)
    expect(reasons[0].textContent).toBe(t('scan.noNrReason.off'))
  })

  it('says Windows may open a harmless browser page, and why it can\'t start yet', () => {
    render(AutoScan, { supported: [1], tones: new Map(), blocked: 'bands.reason.notReady' })
    expect(screen.getByRole('note').textContent).toContain('msftconnecttest')
    expect(screen.getByTestId('scan-blocked').textContent).toBe(t('bands.reason.notReady'))
    expect((screen.getByRole('button', { name: t('scan.start') }) as HTMLButtonElement).disabled).toBe(true)
  })

  it('never tries a downlink-only band alone: Quick skips it, Thorough sends it as an add-on', async () => {
    render(AutoScan, { supported: [1, 32], tones: new Map() })
    const b32 = () => screen.getByLabelText('B32') as HTMLInputElement
    expect(b32().disabled).toBe(true)
    await fireEvent.click(screen.getByRole('button', { name: t('scan.start') }))
    expect(api.startScan).toHaveBeenLastCalledWith('quick', [1], 2, [])

    cleanup()
    resetScan()
    render(AutoScan, { supported: [1, 32], tones: new Map() })
    await fireEvent.click(screen.getByLabelText(t('scan.modes.thorough')))
    expect(b32().disabled).toBe(false)
    expect(b32().checked).toBe(true)
    await fireEvent.click(screen.getByRole('button', { name: t('scan.start') }))
    expect(api.startScan).toHaveBeenLastCalledWith('thorough', [1, 32], 3, [])
  })

  it('needs a band that can work alone', async () => {
    render(AutoScan, { supported: [32], tones: new Map() })
    await fireEvent.click(screen.getByLabelText(t('scan.modes.thorough')))
    expect((screen.getByRole('button', { name: t('scan.start') }) as HTMLButtonElement).disabled).toBe(true)
  })

  it('run again keeps the add-on bands, and counts only bands that can be tried alone', async () => {
    scan.file = file({ mode: 'thorough', addOnBands: [32], queue: [B1B3, B1, B3], results: [result(B1, {}), result(B3, {})] })
    render(AutoScan, { supported: [1, 3, 32], tones: new Map() })
    expect(screen.getByTestId('scan-summary').textContent).toContain(t('scan.summary.triedAll', { n: '⁦2⁩' }))
    await fireEvent.click(screen.getByRole('button', { name: t('scan.runAgain') }))
    expect(api.startScan).toHaveBeenCalledWith('thorough', [1, 3, 32], 1, [])
  })

  it('opens the setup from the results and goes back on Cancel', async () => {
    scan.file = file({ results: [result(B1B3, { isStart: true })] })
    render(AutoScan, { supported: [1, 3], tones: new Map() })
    await fireEvent.click(screen.getByTestId('scan-change-setup'))
    expect(screen.queryByTestId('scan-results')).toBeNull()
    await fireEvent.click(screen.getByTestId('scan-back-results'))
    expect(screen.getByTestId('scan-results')).toBeTruthy()
  })

  it('shows three steps: setup first, scanning while it runs, and a finished scan opens on results', async () => {
    const button = (name: string) => screen.getByRole('button', { name }) as HTMLButtonElement
    const current = () => screen.getByTestId('scan-steps').querySelector('[aria-current="step"]')?.textContent

    render(AutoScan, { supported: [1], tones: new Map() })
    expect(current()).toContain(t('scan.flow.setup'))
    expect(button(t('scan.flow.scanning')).disabled).toBe(true)
    expect(button(t('scan.flow.results')).disabled).toBe(true)
    cleanup()

    scan.file = file({ status: 'running', next: 1, results: [result(B1B3, { isStart: true })] })
    render(AutoScan, { supported: [1, 3], tones: new Map() })
    expect(current()).toContain(t('scan.flow.scanning'))
    expect(button(t('scan.flow.setup')).disabled).toBe(true)
    expect(button(t('scan.flow.results')).disabled).toBe(true)

    scan.file = file({ results: [result(B1B3, { isStart: true })] })
    await waitFor(() => expect(current()).toContain(t('scan.flow.results')))
    await fireEvent.click(button(t('scan.flow.scanning')))
    expect(screen.getByTestId('scan-timeline')).toBeTruthy()
    await fireEvent.click(screen.getByTestId('scan-see-results'))
    expect(current()).toContain(t('scan.flow.results'))
  })

  it('the summary and the table agree: same first row, same numbers, and both columns when history counts', async () => {
    const now = Date.now()
    speed.history = [
      { ...resultFixture({ atMs: now - 86_400_000, provider: 'cloudflare', downloadMbps: 142, uploadMbps: 5 }), allowedBands: B1B3 },
    ]
    scan.file = file({
      startedMs: now - 1000,
      queue: [B1B3, B3],
      results: [
        result(B1B3, { isStart: true, tests: 2, downloadMbps: 32.5, downloadMinMbps: 30, downloadMaxMbps: 35 }),
        result(B3, { tests: 2, downloadMbps: 40, downloadMinMbps: 38, downloadMaxMbps: 42 }),
      ],
    })
    render(AutoScan, { supported: [1, 3, 7], tones: new Map() })
    await fireEvent.click(screen.getByLabelText(t('scan.priority.download')))
    await waitFor(() => expect(screen.getAllByTestId('scan-row')[0].textContent).toMatch(/B1.*\+.*B3/))
    expect(screen.getByRole('columnheader', { name: t('scan.col.today') })).toBeTruthy()
    const avg = screen.getAllByTestId('average-down')[0].textContent ?? ''
    expect(avg).toContain('69.0')
    expect(screen.getAllByTestId('today-down')[0].textContent).toContain('32.5')
    const tiles = screen.getByTestId('scan-pick').textContent ?? ''
    expect(tiles).toContain('69.0')
    expect(tiles).toContain('32.5')
    expect(screen.getByTestId('scan-average-note').textContent).toContain('3')
    speed.history = []
  })

  it('says when automatic tests are in the numbers, and links to speed by band set', async () => {
    resetHistory()
    const now = Date.now()
    speed.history = [
      { ...resultFixture({ atMs: now - 86_400_000, downloadMbps: 142, uploadMbps: 5 }), allowedBands: B3, source: 'auto' },
    ]
    scan.file = file({ startedMs: now - 1000, queue: [B1B3, B3], results: [result(B1B3, { isStart: true }), result(B3)] })
    render(AutoScan, { supported: [1, 3], tones: new Map() })
    expect(screen.getByTestId('scan-includes-auto').textContent).toBe(t('scan.summary.includesAuto'))
    await fireEvent.click(screen.getByRole('button', { name: t('history.link.scan') }))
    expect(app.tab).toBe('history')
    expect(app.historyFocus).toBe('bandSets')
    expect(history.range).toEqual({ kind: '7d' })
    expect(api.applyConfig).not.toHaveBeenCalled()
    speed.history = []
    app.historyFocus = null
    resetHistory()
  })

  it('leaves out the automatic-tests note when none are folded in', () => {
    scan.file = file({ results: [result(B1B3, { isStart: true })] })
    render(AutoScan, { supported: [1, 3], tones: new Map() })
    expect(screen.queryByTestId('scan-includes-auto')).toBeNull()
  })

  it('has no second set of columns without earlier tests', () => {
    scan.file = file({ results: [result(B1B3, { isStart: true })] })
    render(AutoScan, { supported: [1, 3], tones: new Map() })
    expect(screen.queryByRole('columnheader', { name: t('scan.col.today') })).toBeNull()
    expect(screen.queryByTestId('scan-average-note')).toBeNull()
  })

  it('groups the bands to try under FDD, TDD and SDL headings', () => {
    render(AutoScan, { supported: [1, 3, 20, 32, 41, 43], tones: new Map() })
    const group = (id: string) => screen.getByTestId(id)
    expect(group('scan-group-fdd').textContent).toContain(t('bands.group.fdd'))
    expect(group('scan-group-tdd').textContent).toContain(t('bands.group.tdd'))
    expect(group('scan-add-ons').textContent).toContain(t('bands.group.sdl'))
    const bands = (id: string) => [...group(id).querySelectorAll('.pick bdi')].map((b) => b.textContent)
    expect(bands('scan-group-fdd')).toEqual(['B1', 'B3', 'B20'])
    expect(bands('scan-group-tdd')).toEqual(['B41', 'B43'])
    expect(bands('scan-add-ons')).toEqual(['B32'])
    expect(group('scan-group-tdd').querySelector('.pick')?.classList.contains('duplex-tdd')).toBe(true)
  })

  it('All and None change only their own group', async () => {
    render(AutoScan, { supported: [1, 3, 41], tones: new Map() })
    await fireEvent.click(screen.getByRole('button', { name: t('scan.clearGroup', { group: 'FDD' }) }))
    expect((screen.getByLabelText('B1') as HTMLInputElement).checked).toBe(false)
    expect((screen.getByLabelText('B41') as HTMLInputElement).checked).toBe(true)
    await fireEvent.click(screen.getByRole('button', { name: t('scan.selectGroup', { group: 'FDD' }) }))
    expect((screen.getByLabelText('B3') as HTMLInputElement).checked).toBe(true)
  })

  it('suggests combinations of the bands that worked and sends the one picked', async () => {
    render(AutoScan, { supported: [1, 3, 20], tones: new Map([[1, 'good' as const], [3, 'good' as const]]) })
    const suggestions = screen.getAllByTestId('scan-suggestion')
    expect(suggestions).toHaveLength(1)
    await fireEvent.click(suggestions[0])
    expect(suggestions[0].getAttribute('aria-pressed')).toBe('true')
    expect(screen.getByTestId('scan-custom-list').textContent).toMatch(/B1.*B3/)
    expect(screen.getByTestId('scan-breakdown').textContent).toContain(
      t('scan.breakdown', { singles: '⁦3⁩', combos: '⁦1⁩' }),
    )
    await fireEvent.click(screen.getByRole('button', { name: t('scan.start') }))
    expect(api.startScan).toHaveBeenCalledWith('quick', [1, 3, 20], 2, [[1, 3]])
  })

  it('says suggestions need two bands that worked', () => {
    render(AutoScan, { supported: [1, 3], tones: new Map([[1, 'good' as const]]) })
    expect(screen.getByTestId('scan-combos-none').textContent).toBe(t('scan.combos.noneYet'))
  })

  it('says so when the bands that worked are just the current set, and does not urge combinations', () => {
    app.snapshot = { config: { ...start } } as unknown as typeof app.snapshot
    render(AutoScan, { supported: [1, 3], tones: new Map([[1, 'good' as const], [3, 'good' as const]]) })
    expect(screen.getByTestId('scan-combos-none').textContent).toBe(t('scan.combos.onlyCurrent'))
    cleanup()
    scan.file = file({
      queue: [B1B3, B1, B3],
      next: 3,
      results: [
        result(B1B3, { isStart: true, downloadMbps: 30 }),
        result(B1, { downloadMbps: 40 }),
        result(B3, { downloadMbps: 60 }),
      ],
    })
    render(AutoScan, { supported: [1, 3], tones: new Map() })
    expect(screen.getByTestId('scan-summary').textContent).not.toContain(t('scan.summary.noCombos2'))
    expect(screen.queryByTestId('scan-try-combos')).toBeNull()
    app.snapshot = null
  })

  it('can start with combinations only', async () => {
    render(AutoScan, { supported: [1, 3, 41], tones: new Map() })
    for (const group of ['FDD', 'TDD'])
      await fireEvent.click(screen.getByRole('button', { name: t('scan.clearGroup', { group }) }))
    const startButton = () => screen.getByRole('button', { name: t('scan.start') }) as HTMLButtonElement
    expect(startButton().disabled).toBe(true)
    const builder = screen.getByTestId('scan-builder')
    const band = (n: number) => [...builder.querySelectorAll('button')].find((b) => b.textContent === 'B' + n)!
    await fireEvent.click(band(3))
    await fireEvent.click(band(41))
    await fireEvent.click(screen.getByTestId('scan-add-combo'))
    expect(screen.queryByTestId('scan-draft')).toBeNull()
    expect(startButton().disabled).toBe(false)
    await fireEvent.click(startButton())
    expect(api.startScan).toHaveBeenCalledWith('quick', [], 2, [[3, 41]])
  })

  it('the builder says why a combination cannot be added', async () => {
    app.snapshot = { config: { ...start } } as unknown as typeof app.snapshot
    render(AutoScan, { supported: [1, 3, 29, 32], tones: new Map() })
    const builder = screen.getByTestId('scan-builder')
    const band = (n: number) => [...builder.querySelectorAll('button')].find((b) => b.textContent === 'B' + n)!
    const add = () => screen.getByTestId('scan-add-combo') as HTMLButtonElement
    const problem = () => screen.getByTestId('scan-combo-problem').textContent

    await fireEvent.click(band(3))
    expect(add().disabled).toBe(true)
    expect(problem()).toBe(t('scan.combos.needTwo'))
    await fireEvent.click(band(3))

    await fireEvent.click(band(29))
    await fireEvent.click(band(32))
    expect(add().disabled).toBe(true)
    expect(problem()).toBe(t('scan.combos.needUplink'))
    await fireEvent.click(band(29))
    await fireEvent.click(band(32))

    // B1 + B3 is what the router has now.
    await fireEvent.click(band(1))
    await fireEvent.click(band(3))
    expect(add().disabled).toBe(true)
    expect(problem()).toBe(t('scan.combos.isCurrent'))

    await fireEvent.click(band(1))
    await fireEvent.click(band(32))
    expect(add().disabled).toBe(false)
    await fireEvent.click(add())
    await fireEvent.click(band(3))
    await fireEvent.click(band(32))
    expect(problem()).toBe(t('scan.combos.duplicate'))
    app.snapshot = null
  })

  it('run again sends the combinations the user asked for', async () => {
    const B3B20 = (4 | (1 << 19)).toString(16)
    scan.file = file({ queue: [B1B3, B1, B3, B3B20], customSets: [B3B20], results: [result(B1B3, { isStart: true })] })
    render(AutoScan, { supported: [1, 3, 20], tones: new Map() })
    await fireEvent.click(screen.getByRole('button', { name: t('scan.runAgain') }))
    expect(api.startScan).toHaveBeenCalledWith('quick', [1, 3], 1, [[3, 20]])
  })

  it('marks the combinations the user asked for in the table', () => {
    const B3B20 = (4 | (1 << 19)).toString(16)
    scan.file = file({
      queue: [B1B3, B3, B3B20],
      customSets: [B3B20],
      results: [
        result(B1B3, { isStart: true, downloadMbps: 10 }),
        result(B3, { downloadMbps: 20 }),
        result(B3B20, { downloadMbps: 90 }),
      ],
    })
    render(AutoScan, { supported: [1, 3, 20], tones: new Map() })
    const tagged = screen.getAllByTestId('scan-row').filter((r) => r.textContent?.includes(t('scan.yourCombo')))
    expect(tagged).toHaveLength(1)
    expect(tagged[0].textContent).toMatch(/B3.*B20/)
    // Combinations were tested, so there's no offer to test them.
    expect(screen.queryByTestId('scan-try-combos')).toBeNull()
  })

  it('offers to test combinations of the bands that worked after a Quick scan', async () => {
    const B20 = (1 << 19).toString(16)
    scan.file = file({
      queue: [B1B3, B1, B3, B20],
      next: 4,
      results: [
        result(B1B3, { isStart: true, downloadMbps: 30 }),
        result(B1, { downloadMbps: 40 }),
        result(B3, { downloadMbps: 60 }),
        result(B20, { outcome: 'noService', downloadMbps: null, uploadMbps: null }),
      ],
    })
    render(AutoScan, { supported: [1, 3, 20], tones: new Map() })
    expect(screen.getByTestId('scan-summary').textContent).toContain(t('scan.summary.noCombos2'))
    await fireEvent.click(screen.getByTestId('scan-try-combos'))
    expect(screen.queryByTestId('scan-results')).toBeNull()
    expect(screen.getByTestId('scan-custom-list').textContent).toMatch(/B1.*B3/)
    for (const b of ['B1', 'B3', 'B20']) expect((screen.getByLabelText(b) as HTMLInputElement).checked).toBe(false)
    await fireEvent.click(screen.getByRole('button', { name: t('scan.start') }))
    expect(api.startScan).toHaveBeenCalledWith('quick', [], 2, [[1, 3]])
  })

  it('warns when it could not get back', () => {
    scan.file = file({ returnFailed: true })
    render(AutoScan, { supported: [1, 3], tones: new Map() })
    expect(screen.getByRole('alert').textContent).toContain(t('scan.returnFailed'))
  })
})
