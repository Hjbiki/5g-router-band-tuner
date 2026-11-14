import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))
vi.mock('../lib/backend', () => ({
  api: {
    previewConfig: vi.fn(),
    applyConfig: vi.fn(),
    saveSettings: vi.fn(),
    bandOutcomes: vi.fn(),
    scanPeriods: vi.fn(),
    speedTestHistory: vi.fn(),
    speedTestRunning: vi.fn(),
    getScan: vi.fn(),
  },
  onSpeedTest: vi.fn(),
  onScan: vi.fn(),
}))

import { api, onScan, onSpeedTest } from '../lib/backend'
import { app, bandEdit } from '../lib/store.svelte'
import { t } from '../lib/i18n'
import BandPanel from './BandPanel.svelte'
import { settingsFixture, snap } from '../test-fixtures'
import { isolate } from '../lib/format'

const BODY = '<request><NetworkMode>00</NetworkMode><LTEBand>5</LTEBand></request>'

describe('BandPanel', () => {
  beforeEach(() => {
    Object.assign(bandEdit, { draft: null, dirty: false, wasClosed: true, primaryEdited: false })
    app.snapshot = snap()
    app.settings = null
    vi.mocked(api.previewConfig).mockResolvedValue({ fields: [['LTEBand', '5']], body: BODY })
    vi.mocked(api.applyConfig).mockResolvedValue(undefined)
    vi.mocked(api.bandOutcomes).mockResolvedValue([])
    vi.mocked(api.scanPeriods).mockResolvedValue([])
    vi.mocked(api.speedTestHistory).mockResolvedValue([])
    vi.mocked(api.speedTestRunning).mockResolvedValue(false)
    vi.mocked(onSpeedTest).mockResolvedValue(() => {})
    vi.mocked(onScan).mockResolvedValue(() => {})
    vi.mocked(api.getScan).mockResolvedValue(null)
  })
  afterEach(() => {
    cleanup()
    vi.clearAllMocks()
  })

  it('renders chips for supported bands only', () => {
    render(BandPanel)
    expect(screen.getByRole('button', { name: /^B3\b/ })).toBeTruthy()
    expect(screen.getByRole('button', { name: /^B43\b/ })).toBeTruthy()
    expect(screen.queryByRole('button', { name: /^B2\b/ })).toBeNull()
    expect(screen.queryByRole('button', { name: /^B4\b/ })).toBeNull()
    expect(screen.queryByRole('button', { name: /^B66\b/ })).toBeNull()
  })

  it('disables Apply with the reason when band editing is not allowed', () => {
    app.snapshot = snap({
      capabilities: { ...snap().capabilities!, bandEditAllowed: false },
    })
    render(BandPanel)
    const apply = screen.getByRole('button', { name: t('bands.applySafely') }) as HTMLButtonElement
    expect(apply.disabled).toBe(true)
    expect(screen.getByText(t('bands.reason.editNotAllowed'))).toBeTruthy()
  })

  it('disables Apply with a reason when nothing changed', () => {
    render(BandPanel)
    expect((screen.getByRole('button', { name: t('bands.applySafely') }) as HTMLButtonElement).disabled).toBe(true)
    expect(screen.getByText(t('bands.reason.unchanged'))).toBeTruthy()
  })

  it('a star on the same bands can be applied, and warns that everyone loses internet', async () => {
    app.settings = settingsFixture()
    vi.mocked(api.saveSettings).mockResolvedValue(undefined)
    render(BandPanel)
    const apply = screen.getByRole('button', { name: t('bands.applySafely') }) as HTMLButtonElement
    expect(apply.disabled).toBe(true)
    expect(screen.queryByText(t('bands.change.everyone'))).toBeNull()

    // B3 is in the router's current set, so the bands stay as they are.
    await fireEvent.click(screen.getByRole('button', { name: t('bands.makePrimary', { band: 'B3' }) }))
    await waitFor(() => expect(apply.disabled).toBe(false))
    expect(screen.getByText(t('bands.change.everyone'))).toBeTruthy()
    await fireEvent.click(apply)
    expect(api.applyConfig).toHaveBeenCalledWith(expect.objectContaining({ lteBands: snap().config!.lteBands }))
  })

  it('shows exactly the body returned by previewConfig', async () => {
    render(BandPanel)
    await fireEvent.click(screen.getByRole('button', { name: /^B3\b/ }))
    await waitFor(() => expect(screen.getByTestId('preview-body').textContent).toBe(BODY))
    expect(api.previewConfig).toHaveBeenLastCalledWith(expect.objectContaining({ lteBands: '1' }))
    expect(screen.getByTestId('preview-body').getAttribute('dir')).toBe('ltr')
  })

  it('applies the edited config once the preview is current', async () => {
    render(BandPanel)
    await fireEvent.click(screen.getByRole('button', { name: /^B7\b/ }))
    const apply = screen.getByRole('button', { name: t('bands.applySafely') }) as HTMLButtonElement
    await waitFor(() => expect(apply.disabled).toBe(false))
    await fireEvent.click(apply)
    expect(api.applyConfig).toHaveBeenCalledWith(expect.objectContaining({ lteBands: '45' }))
  })

  it('hides "Discard edits" once the edits are undone', async () => {
    render(BandPanel)
    expect(screen.queryByRole('button', { name: t('bands.resetEdits') })).toBeNull()
    await fireEvent.click(screen.getByRole('button', { name: /^B7\b/ }))
    expect(screen.getByRole('button', { name: t('bands.resetEdits') })).toBeTruthy()
    await fireEvent.click(screen.getByRole('button', { name: /^B7\b/ }))
    expect(screen.queryByRole('button', { name: t('bands.resetEdits') })).toBeNull()
  })

  it('keeps unsaved edits when the panel is closed and reopened (tab switch)', async () => {
    render(BandPanel)
    await fireEvent.click(screen.getByRole('button', { name: /^B7\b/ }))
    expect(screen.getByRole('button', { name: t('bands.resetEdits') })).toBeTruthy()
    cleanup() // switch to another tab
    render(BandPanel) // and back
    expect(screen.getByRole('button', { name: t('bands.resetEdits') })).toBeTruthy()
    expect(screen.getByRole('button', { name: /^B7\b/ }).getAttribute('aria-pressed')).toBe('true')
  })

  it('blocks Apply when every band is deselected', async () => {
    app.snapshot = snap({ config: { ...snap().config!, lteBands: '1' } })
    render(BandPanel)
    await fireEvent.click(screen.getByRole('button', { name: /^B1\b/ }))
    expect((screen.getByRole('button', { name: t('bands.applySafely') }) as HTMLButtonElement).disabled).toBe(true)
    expect(screen.getByText(t('bands.reason.noBands'))).toBeTruthy()
  })

  it('warns when NR-only is combined with NSA', async () => {
    app.snapshot = snap({
      config: { networkMode: { kind: 'nrOnly' }, lteBands: '5', legacyBandRaw: '0', nrOption: 'nsa' },
    })
    render(BandPanel)
    expect(screen.getByText(t('bands.nsaNeedsAnchor'))).toBeTruthy()
  })

  it('disables Apply with a reason while a foreign journal exists', () => {
    app.snapshot = snap({ foreignJournal: {} as never })
    render(BandPanel)
    expect((screen.getByRole('button', { name: t('bands.applySafely') }) as HTMLButtonElement).disabled).toBe(true)
    expect(screen.getByText(t('bands.reason.foreignJournal'))).toBeTruthy()
  })

  it('does not refetch the preview for identical snapshots', async () => {
    render(BandPanel)
    await waitFor(() => expect(api.previewConfig).toHaveBeenCalledTimes(1))
    app.snapshot = snap()
    app.snapshot = snap()
    await new Promise((r) => setTimeout(r, 400))
    expect(api.previewConfig).toHaveBeenCalledTimes(1)
  })

  it('disables Apply while a safe-apply is active', () => {
    app.snapshot = snap({
      safeApply: { phase: { kind: 'probation' }, secondsLeft: 100, previous: null, requested: null },
    })
    render(BandPanel)
    expect((screen.getByRole('button', { name: t('bands.applySafely') }) as HTMLButtonElement).disabled).toBe(true)
    expect(screen.getByText(t('bands.reason.changeInProgress'))).toBeTruthy()
  })

  it('starring a band saves it as the primary band and selects it', async () => {
    app.settings = settingsFixture()
    vi.mocked(api.saveSettings).mockResolvedValue(undefined)
    render(BandPanel)
    const star = screen.getByRole('button', { name: t('bands.makePrimary', { band: 'B7' }) })
    expect(star.getAttribute('aria-pressed')).toBe('false')
    await fireEvent.click(star)
    expect(api.saveSettings).toHaveBeenCalledWith(
      expect.objectContaining({ automation: expect.objectContaining({ primaryBand: 7 }) }),
    )
    await waitFor(() => expect(app.settings?.automation.primaryBand).toBe(7))
    expect(star.getAttribute('aria-pressed')).toBe('true')
    // B7 was not selected, so starring it selected it.
    expect(screen.getByRole('button', { name: /^B7\b/ }).getAttribute('aria-pressed')).toBe('true')
    await waitFor(() => expect(api.previewConfig).toHaveBeenLastCalledWith(expect.objectContaining({ lteBands: '45' })))
  })

  it('starring the primary band again clears it and keeps the selection', async () => {
    app.settings = settingsFixture({ automation: { primaryBand: 3, relockPrimary: false, revertOnLoss: true } })
    vi.mocked(api.saveSettings).mockResolvedValue(undefined)
    render(BandPanel)
    const star = screen.getByRole('button', { name: t('bands.makePrimary', { band: 'B3' }) })
    expect(star.getAttribute('aria-pressed')).toBe('true')
    await fireEvent.click(star)
    expect(api.saveSettings).toHaveBeenCalledWith(
      expect.objectContaining({ automation: expect.objectContaining({ primaryBand: null }) }),
    )
    await waitFor(() => expect(star.getAttribute('aria-pressed')).toBe('false'))
    expect(screen.getByRole('button', { name: /^B3\b/ }).getAttribute('aria-pressed')).toBe('true')
  })

  it('a failed primary-band save shows a toast and keeps the old setting', async () => {
    app.settings = settingsFixture()
    app.toasts = []
    vi.mocked(api.saveSettings).mockRejectedValueOnce('disk full')
    render(BandPanel)
    await fireEvent.click(screen.getByRole('button', { name: t('bands.makePrimary', { band: 'B3' }) }))
    await waitFor(() => expect(app.toasts.some((x) => x.kind === 'error')).toBe(true))
    expect(app.settings?.automation.primaryBand).toBeNull()
  })

  it('clearing the star also turns re-lock off in the same save', async () => {
    app.settings = settingsFixture({ automation: { primaryBand: 3, relockPrimary: true, revertOnLoss: true } })
    vi.mocked(api.saveSettings).mockResolvedValue(undefined)
    render(BandPanel)
    await fireEvent.click(screen.getByRole('button', { name: t('bands.makePrimary', { band: 'B3' }) }))
    await waitFor(() => expect(api.saveSettings).toHaveBeenCalledOnce())
    expect(vi.mocked(api.saveSettings).mock.calls[0][0].automation).toEqual({
      primaryBand: null,
      relockPrimary: false,
      revertOnLoss: true,
    })
    await waitFor(() => expect(app.settings?.automation.relockPrimary).toBe(false))
  })

  it('starring a band keeps the stored re-lock choice', async () => {
    app.settings = settingsFixture({ automation: { primaryBand: 3, relockPrimary: true, revertOnLoss: true } })
    vi.mocked(api.saveSettings).mockResolvedValue(undefined)
    render(BandPanel)
    await fireEvent.click(screen.getByRole('button', { name: t('bands.makePrimary', { band: 'B1' }) }))
    await waitFor(() => expect(api.saveSettings).toHaveBeenCalledOnce())
    expect(vi.mocked(api.saveSettings).mock.calls[0][0].automation.relockPrimary).toBe(true)
  })

  it('a star and a preset saved at the same time both stick', async () => {
    app.settings = settingsFixture()
    let release!: () => void
    vi.mocked(api.saveSettings)
      .mockImplementationOnce(() => new Promise<void>((r) => (release = r)))
      .mockResolvedValue(undefined)
    render(BandPanel)
    await fireEvent.click(screen.getByRole('button', { name: t('bands.makePrimary', { band: 'B3' }) }))
    await waitFor(() => expect(api.saveSettings).toHaveBeenCalledOnce())
    // The star's save is still on its way when the preset is saved.
    await fireEvent.input(screen.getByLabelText(t('bands.presetName')), { target: { value: 'Home' } })
    await fireEvent.click(screen.getByRole('button', { name: t('bands.savePreset') }))
    await new Promise((r) => setTimeout(r, 0))
    expect(api.saveSettings).toHaveBeenCalledOnce()
    release()
    await waitFor(() => expect(api.saveSettings).toHaveBeenCalledTimes(2))
    const second = vi.mocked(api.saveSettings).mock.calls[1][0]
    expect(second.automation.primaryBand).toBe(3)
    expect(second.presets.map((p) => p.name)).toEqual(['Home'])
    await waitFor(() => expect(app.settings?.presets.map((p) => p.name)).toEqual(['Home']))
    expect(app.settings?.automation.primaryBand).toBe(3)
  })

  it('stays locked between the two steps of a primary-band apply and keeps the edits', async () => {
    // The user applied B1+B3+B7 with B3 starred; step 1 (B3 alone) is kept, step 2 is next.
    Object.assign(bandEdit, {
      draft: { ...snap().config!, lteBands: '45' },
      dirty: true,
      wasClosed: false,
    })
    app.settings = settingsFixture({ automation: { primaryBand: 3, relockPrimary: false, revertOnLoss: true } })
    app.snapshot = snap({
      config: { ...snap().config!, lteBands: '4' },
      safeApply: {
        phase: { kind: 'committed' },
        secondsLeft: null,
        previous: null,
        requested: null,
        sequence: { step: 1, total: 2, primary: 3, finalConfig: { ...snap().config!, lteBands: '45' }, origin: 'user' },
      },
    })
    render(BandPanel)
    expect((screen.getByRole('button', { name: t('bands.applySafely') }) as HTMLButtonElement).disabled).toBe(true)
    expect(screen.getByText(t('bands.reason.changeInProgress'))).toBeTruthy()
    expect((screen.getByRole('button', { name: /^B7\b/ }) as HTMLButtonElement).disabled).toBe(true)
    expect(bandEdit.dirty).toBe(true)
    expect(screen.getByRole('button', { name: /^B7\b/ }).getAttribute('aria-pressed')).toBe('true')
  })

  const timers = { graceS: 120, lossS: 60, probationS: 180 }
  const twoStepLine = (band: string) =>
    t('bands.change.stepTwoStep', { band: isolate(band), grace: t('bands.duration.min', { m: isolate('2') }) })

  it('says the apply runs in two steps when the starred band and others are selected', async () => {
    app.settings = settingsFixture({ timers, automation: { primaryBand: 3, relockPrimary: false, revertOnLoss: true } })
    render(BandPanel)
    await fireEvent.click(screen.getByRole('button', { name: /^B7\b/ }))
    expect(screen.getByText(twoStepLine('B3'))).toBeTruthy()
  })

  it('no two-step line without a star, or with the starred band alone', () => {
    const line = twoStepLine
    app.settings = settingsFixture({ timers })
    render(BandPanel)
    expect(screen.queryByText(line('B3'))).toBeNull()
    cleanup()

    Object.assign(bandEdit, { draft: null, dirty: false, wasClosed: true, primaryEdited: false })
    app.settings = settingsFixture({ timers, automation: { primaryBand: 3, relockPrimary: false, revertOnLoss: true } })
    app.snapshot = snap({ config: { ...snap().config!, lteBands: '4' } })
    render(BandPanel)
    expect(screen.queryByText(line('B3'))).toBeNull()
  })

  it('marks the serving band and what changed against the router', async () => {
    app.snapshot = snap({ signal: { serving: 'lte', lte: { band: 3 }, nr: null } as never })
    render(BandPanel)
    expect(screen.getByRole('button', { name: /^B3\b/ }).textContent).toContain(t('bands.servingNow'))
    await fireEvent.click(screen.getByRole('button', { name: /^B7\b/ }))
    await fireEvent.click(screen.getByRole('button', { name: /^B1\b/ }))
    expect(screen.getByRole('button', { name: /^B7\b/ }).textContent).toContain(t('bands.added'))
    expect(screen.getByRole('button', { name: /^B1\b/ }).textContent).toContain(t('bands.removed'))
    expect(screen.getByRole('button', { name: /^B3\b/ }).textContent).not.toContain(t('bands.added'))
  })

  it('counts the selected bands per group', () => {
    render(BandPanel)
    const count = (id: string) => screen.getByTestId(id).querySelector('.count')?.textContent
    expect(count('band-group-fdd')).toBe(t('bands.selectedCount', { n: isolate('2'), total: isolate('7') }))
    expect(count('band-group-tdd')).toBe(t('bands.selectedCount', { n: isolate('0'), total: isolate('7') }))
    expect(count('band-group-sdl')).toBe(t('bands.selectedCount', { n: isolate('0'), total: isolate('1') }))
  })

  it('has FDD, TDD and SDL groups, each band striped with its kind', () => {
    render(BandPanel)
    for (const d of ['fdd', 'tdd', 'sdl'] as const)
      expect(screen.getByTestId(`band-group-${d}`).querySelector('h3')?.textContent).toBe(t(`bands.group.${d}`))
    const sdl = screen.getByTestId('band-group-sdl')
    const b32 = screen.getByRole('button', { name: /^B32\b/ })
    expect(sdl.contains(b32)).toBe(true)
    expect(b32.classList.contains('duplex-sdl')).toBe(true)
    expect(screen.getByTestId('band-group-fdd').contains(b32)).toBe(false)
    expect(screen.getByRole('button', { name: /^B41\b/ }).classList.contains('duplex-tdd')).toBe(true)
  })

  it('explains the apply steps with the user timers, once there is a change to apply', async () => {
    app.settings = settingsFixture({ timers: { graceS: 120, lossS: 45, probationS: 300 } })
    render(BandPanel)
    // Nothing to apply yet: the panel says so and keeps the steps out of the way.
    expect(screen.getByTestId('change-idle')).toBeTruthy()
    expect(screen.queryByTestId('apply-steps')).toBeNull()
    await fireEvent.click(screen.getByRole('button', { name: /^B7\b/ }))
    expect(screen.queryByTestId('change-idle')).toBeNull()
    expect(screen.getByTestId('band-delta').textContent).toContain('+B7')
    const min = (m: number) => t('bands.duration.min', { m: isolate(String(m)) })
    expect(
      screen.getByText(t('bands.change.stepReconnect', { grace: min(2) })),
    ).toBeTruthy()
    expect(
      screen.getByText(
        t('bands.change.stepTrial', { probation: min(5), loss: t('bands.duration.s', { s: isolate('45') }) }),
      ),
    ).toBeTruthy()
  })

  it('says plainly that the change will not revert by itself when automatic revert is off', async () => {
    app.settings = settingsFixture({ automation: { primaryBand: null, relockPrimary: false, revertOnLoss: false } })
    render(BandPanel)
    await fireEvent.click(screen.getByRole('button', { name: /^B7\b/ }))
    const probation = t('bands.duration.min', { m: isolate('3') })
    expect(screen.getByText(t('bands.change.stepTrialNoRevert', { probation }))).toBeTruthy()
  })

  it('the Auto preset keeps the current 5G option', async () => {
    app.snapshot = snap({
      config: { networkMode: { kind: 'lteOnly' }, lteBands: '5', legacyBandRaw: '0', nrOption: 'sa' },
    })
    render(BandPanel)
    await fireEvent.click(screen.getByRole('button', { name: t('bands.presetAuto') }))
    await waitFor(() =>
      expect(api.previewConfig).toHaveBeenLastCalledWith(
        expect.objectContaining({ networkMode: { kind: 'auto' }, nrOption: 'sa' }),
      ),
    )
  })

  it('tints a band by its last result and explains the colours', async () => {
    vi.mocked(api.bandOutcomes).mockResolvedValue([{ atMs: 5, band: 3, kind: 'noService' }])
    render(BandPanel)
    const chip = await screen.findByRole('button', { name: /^B1\b/ })
    expect(chip.getAttribute('title')).toBeNull()
    await waitFor(() => expect(screen.getByRole('button', { name: /^B3\b/ }).getAttribute('title')).toBe(t('bands.legend.noService')))
    // The legend explains only the marks on screen.
    const legend = screen.getByRole('list', { name: t('bands.legend.label') }).textContent
    expect(legend).toContain(t('bands.legend.noService'))
    expect(legend).not.toContain(t('bands.legend.good'))
  })

  it('a chip\'s i shows its last result, even while the panel is locked', async () => {
    vi.mocked(api.bandOutcomes).mockResolvedValue([{ atMs: 5, band: 3, kind: 'noService' }])
    vi.mocked(api.scanPeriods).mockResolvedValue([[1, 9]])
    app.snapshot = snap({ safeApply: { phase: { kind: 'probation' }, secondsLeft: 9, previous: null, requested: null } as never })
    render(BandPanel)
    await screen.findByRole('button', { name: /^B3\b/ })
    await waitFor(() => expect(screen.getByRole('button', { name: /^B3\b/ }).getAttribute('title')).toBe(t('bands.legend.noService')))
    const info = screen.getByRole('button', { name: t('bands.last.label', { band: 'B3' }) }) as HTMLButtonElement
    expect(info.disabled).toBe(false)
    await fireEvent.click(info)
    const shown = screen.getByTestId('band-last-result').textContent
    expect(shown).toContain(t('bands.last.noService'))
    expect(shown).toContain(t('bands.last.fromScan'))
  })

  it('says a band with no history has none, and why B32 can\'t work alone', async () => {
    render(BandPanel)
    await fireEvent.click(await screen.findByRole('button', { name: t('bands.last.label', { band: 'B32' }) }))
    expect(screen.getByTestId('band-add-on').textContent).toBe(t('bands.last.addOn'))
    expect(screen.getByTestId('band-last-result').textContent).toContain(t('bands.last.none'))
  })

  it('marks the band in use inside the chip, with no second outline class', async () => {
    app.snapshot = snap({ signal: { serving: 'lte', lte: { band: 1 }, nr: null } as never })
    render(BandPanel)
    const chip = await screen.findByRole('button', { name: /^B1\b/ })
    expect(chip.textContent).toContain(t('bands.servingNow'))
    expect(chip.querySelector('.serving svg')).not.toBeNull()
    expect(chip.getAttribute('title')).toBe(t('bands.servingNow'))
    expect(chip.classList.contains('now')).toBe(false)
  })
})
