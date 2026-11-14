import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))
vi.mock('../lib/backend', () => ({
  api: {
    saveSettings: vi.fn(),
    saveCredentials: vi.fn(),
    forgetCredentials: vi.fn(),
    connect: vi.fn(),
    credentialStatus: vi.fn(),
    autoSpeedTestStatus: vi.fn(),
    historyOverview: vi.fn(),
    clearHistory: vi.fn(),
  },
  onAutoSpeedTest: vi.fn(),
}))

import { api, onAutoSpeedTest } from '../lib/backend'
import { app } from '../lib/store.svelte'
import { LANGUAGES, t } from '../lib/i18n'
import Settings from './Settings.svelte'
import { credentials } from '../lib/credentials.svelte'
import ForeignJournalNotice from './ForeignJournalNotice.svelte'
import { snap } from '../test-fixtures'
import { settingsFixture } from '../test-fixtures'
import type { Settings as S } from '../lib/bindings/Settings'
import type { AutoSpeedTestStatus } from '../lib/bindings/AutoSpeedTestStatus'
import { overviewFixture } from '../lib/history.fixtures'

const autoStatus = (o: Partial<AutoSpeedTestStatus> = {}): AutoSpeedTestStatus => ({
  enabled: false,
  nextSlotMs: null,
  lastAttemptMs: null,
  lastOutcome: null,
  lastSkipReason: null,
  blockedBy: null,
  running: false,
  ...o,
})

const base: S = settingsFixture()

describe('Settings', () => {
  beforeEach(() => {
    app.settings = structuredClone(base)
    vi.mocked(api.saveSettings).mockResolvedValue(undefined)
    vi.mocked(api.saveCredentials).mockResolvedValue(undefined)
    vi.mocked(api.autoSpeedTestStatus).mockResolvedValue(autoStatus())
    vi.mocked(api.historyOverview).mockResolvedValue(overviewFixture({ dbBytes: 2_500_000 }))
    vi.mocked(api.clearHistory).mockResolvedValue(undefined)
    vi.mocked(onAutoSpeedTest).mockResolvedValue(() => {})
  })
  afterEach(() => {
    cleanup()
    vi.clearAllMocks()
  })

  it('leaves the password and log in to the Router page', () => {
    app.snapshot = snap({ session: { kind: 'notConfigured' } })
    credentials.status = { saved: false, rejected: false }
    render(Settings)
    expect(screen.queryByLabelText(t('settings.password'))).toBeNull()
    expect(screen.getByText(t('settings.connectionHint'))).toBeTruthy()
  })

  it('says an edited address or username must be saved before logging in', async () => {
    app.snapshot = snap({ session: { kind: 'notConfigured' } })
    credentials.status = { saved: false, rejected: false }
    render(Settings)
    expect(screen.queryByTestId('login-account-dirty')).toBeNull()
    await fireEvent.input(screen.getByLabelText(t('settings.username')), { target: { value: 'other' } })
    expect(screen.getByTestId('login-account-dirty')).toBeTruthy()
  })

  it('shows an inline range error and blocks saving', async () => {
    render(Settings)
    await fireEvent.input(screen.getByLabelText(t('settings.reattachGrace')), { target: { value: '10' } })
    expect(screen.getAllByRole('alert').length).toBeGreaterThan(0)
    expect((screen.getByRole('button', { name: t('settings.saveChanges') }) as HTMLButtonElement).disabled).toBe(true)
  })

  it('surfaces the backend refusal in the user language', async () => {
    vi.mocked(api.saveSettings).mockRejectedValue(
      'Finish or revert the current band change before changing router or driver.',
    )
    render(Settings)
    await fireEvent.click(screen.getByLabelText(t('appearance.cobalt')))
    await fireEvent.click(screen.getByRole('button', { name: t('settings.saveChanges') }))
    await waitFor(() => expect(screen.getByText(t('settings.error.changeInProgress'))).toBeTruthy())
  })

  it('switches show their stored state in words, not colour alone', async () => {
    render(Settings)
    const revert = screen.getByRole('switch', { name: t('automation.revertOnLoss') })
    expect(revert.getAttribute('aria-checked')).toBe('true')
    expect(revert.textContent).toBe(t('settings.switchOn'))
    const cell = screen.getByRole('switch', { name: t('automation.cellAlerts') })
    expect(cell.getAttribute('aria-checked')).toBe('true')
    expect(cell.textContent).toBe(t('settings.switchOn'))
  })

  it('asks before turning auto-undo off, then warns until it is back on', async () => {
    render(Settings)
    const sw = screen.getByRole('switch', { name: t('automation.revertOnLoss') })
    expect(sw.getAttribute('aria-checked')).toBe('true')

    await fireEvent.click(sw)
    // Nothing changes until confirmed.
    expect(sw.getAttribute('aria-checked')).toBe('true')
    await fireEvent.click(screen.getByRole('button', { name: t('app.cancel') }))
    expect(sw.getAttribute('aria-checked')).toBe('true')
    expect(screen.queryByText(t('automation.offTitle'))).toBeNull()

    await fireEvent.click(sw)
    await fireEvent.click(screen.getByRole('button', { name: t('automation.confirmOff') }))
    expect(sw.getAttribute('aria-checked')).toBe('false')
    expect(screen.getByRole('alert').textContent).toContain(t('automation.offTitle'))

    await fireEvent.click(screen.getByRole('button', { name: t('settings.saveChanges') }))
    await waitFor(() => expect(api.saveSettings).toHaveBeenCalledOnce())
    expect(vi.mocked(api.saveSettings).mock.calls[0][0].automation.revertOnLoss).toBe(false)

    await fireEvent.click(screen.getByRole('button', { name: t('automation.turnBackOn') }))
    expect(sw.getAttribute('aria-checked')).toBe('true')
    expect(screen.queryByText(t('automation.offTitle'))).toBeNull()
  })

  it('keeps the primary-band switch disabled until a band is starred', async () => {
    render(Settings)
    const sw = screen.getByRole('switch', { name: t('automation.relock') }) as HTMLButtonElement
    expect(sw.disabled).toBe(true)
    expect(screen.getByText(t('automation.noPrimary'))).toBeTruthy()
    cleanup()

    app.settings = structuredClone({ ...base, automation: { ...base.automation, primaryBand: 3 } })
    render(Settings)
    const on = screen.getByRole('switch', { name: /B3/ }) as HTMLButtonElement
    expect(on.disabled).toBe(false)
    await fireEvent.click(on)
    await fireEvent.click(screen.getByRole('button', { name: t('settings.saveChanges') }))
    await waitFor(() => expect(api.saveSettings).toHaveBeenCalledOnce())
    expect(vi.mocked(api.saveSettings).mock.calls[0][0].automation).toEqual({
      primaryBand: 3,
      relockPrimary: true,
      revertOnLoss: true,
    })
  })

  it('previews theme and palette at once, saves them, and restores the stored look when left unsaved', async () => {
    const root = document.documentElement
    const view = render(Settings)
    await fireEvent.click(screen.getByLabelText(t('appearance.themeDark')))
    await fireEvent.click(screen.getByLabelText(t('appearance.sandstone')))
    expect(root.dataset.theme).toBe('dark')
    expect(root.dataset.palette).toBe('sandstone')
    expect(api.saveSettings).not.toHaveBeenCalled()

    view.unmount()
    expect(root.dataset.palette).toBe('carbon')

    render(Settings)
    await fireEvent.click(screen.getByLabelText(t('appearance.cobalt')))
    await fireEvent.click(screen.getByRole('button', { name: t('settings.saveChanges') }))
    await waitFor(() => expect(api.saveSettings).toHaveBeenCalledOnce())
    expect(vi.mocked(api.saveSettings).mock.calls[0][0].palette).toBe('cobalt')
    expect(app.settings?.palette).toBe('cobalt')
  })

  it('Save keeps a Switch back made while Settings was open', async () => {
    app.snapshot = snap({ foreignJournal: { host: '10.0.0.9', driver: 'mock' } as never })
    render(ForeignJournalNotice)
    render(Settings)
    // The user edits one field, then switches back to the journal's router from the notice.
    await fireEvent.click(screen.getByRole('switch', { name: t('automation.cellAlerts') }))
    await fireEvent.click(screen.getByRole('button', { name: t('foreign.switchBack') }))
    await waitFor(() => expect(app.settings?.host).toBe('10.0.0.9'))
    // Untouched fields follow the new settings.
    await waitFor(() => expect((screen.getByLabelText(t('settings.host')) as HTMLInputElement).value).toBe('10.0.0.9'))
    expect((screen.getByLabelText(t('settings.driver')) as HTMLSelectElement).value).toBe('mock')
    vi.mocked(api.saveSettings).mockClear()

    await fireEvent.click(screen.getByRole('button', { name: t('settings.saveChanges') }))
    await waitFor(() => expect(api.saveSettings).toHaveBeenCalledOnce())
    const saved = vi.mocked(api.saveSettings).mock.calls[0][0]
    expect(saved.host).toBe('10.0.0.9')
    expect(saved.driver).toBe('mock')
    expect(saved.cellAlerts).toBe(false)
    app.snapshot = null
  })

  it('Save keeps a primary band starred on the Bands page while Settings was open', async () => {
    render(Settings)
    await fireEvent.click(screen.getByLabelText(t('appearance.cobalt')))
    // What BandPanel's star does: a save of its own, straight into the store.
    app.settings = { ...structuredClone(base), automation: { ...base.automation, primaryBand: 7 } }
    await waitFor(() => expect(screen.getByRole('switch', { name: /B7/ })).toBeTruthy())
    // A change from elsewhere alone doesn't make the form "unsaved".
    expect((screen.getByLabelText(t('appearance.cobalt')) as HTMLInputElement).checked).toBe(true)

    await fireEvent.click(screen.getByRole('button', { name: t('settings.saveChanges') }))
    await waitFor(() => expect(api.saveSettings).toHaveBeenCalledOnce())
    const saved = vi.mocked(api.saveSettings).mock.calls[0][0]
    expect(saved.automation.primaryBand).toBe(7)
    expect(saved.palette).toBe('cobalt')
  })

  it("an edit the user made survives a settings change from elsewhere, and isn't unsaved when there is none", async () => {
    render(Settings)
    const host = screen.getByLabelText(t('settings.host')) as HTMLInputElement
    await fireEvent.input(host, { target: { value: '192.168.1.1' } })
    app.settings = { ...structuredClone(base), cellAlerts: false }
    await waitFor(() =>
      expect(screen.getByRole('switch', { name: t('automation.cellAlerts') }).getAttribute('aria-checked')).toBe('false'),
    )
    expect(host.value).toBe('192.168.1.1')
    expect(screen.getByTestId('unsaved').textContent).toBe(t('settings.unsavedOne', { what: t('settings.host') }))
    await fireEvent.input(host, { target: { value: base.host } })
    await waitFor(() => expect(screen.queryByTestId('unsaved')).toBeNull())
  })

  it('the re-lock switch shows the stored value, even without a primary band', () => {
    app.settings = { ...structuredClone(base), automation: { ...base.automation, primaryBand: null, relockPrimary: true } }
    render(Settings)
    const sw = screen.getByRole('switch', { name: t('automation.relock') }) as HTMLButtonElement
    expect(sw.getAttribute('aria-checked')).toBe('true')
    expect(sw.disabled).toBe(true)
  })

  it('offers System and every language', () => {
    render(Settings)
    const select = screen.getByLabelText(t('settings.language')) as HTMLSelectElement
    const values = [...select.options].map((o) => o.value)
    expect(values[0]).toBe('system')
    for (const l of LANGUAGES) expect(values).toContain(l.code)
  })
  it('shows the save bar only with unsaved changes, names them, and Discard undoes them', async () => {
    render(Settings)
    expect(screen.queryByRole('button', { name: t('settings.saveChanges') })).toBeNull()
    await fireEvent.click(screen.getByLabelText('125%'))
    expect(screen.getByTestId('unsaved').textContent).toBe(t('settings.unsavedOne', { what: t('settings.uiScale') }))
    await fireEvent.click(screen.getByLabelText(t('appearance.cobalt')))
    expect(screen.getByTestId('unsaved').textContent).toContain(t('appearance.palette'))
    await fireEvent.click(screen.getByRole('button', { name: t('settings.discard') }))
    expect(screen.queryByTestId('unsaved')).toBeNull()
    expect((screen.getByLabelText('100%') as HTMLInputElement).checked).toBe(true)
    expect(api.saveSettings).not.toHaveBeenCalled()
  })

  it('saves the UI size and keeps it out of the way of other edits', async () => {
    render(Settings)
    await fireEvent.click(screen.getByLabelText('110%'))
    await fireEvent.click(screen.getByRole('button', { name: t('settings.saveChanges') }))
    await waitFor(() => expect(api.saveSettings).toHaveBeenCalledOnce())
    expect(vi.mocked(api.saveSettings).mock.calls[0][0].uiScale).toBe(110)
  })

  it('explains every timer', () => {
    render(Settings)
    for (const key of ['settings.reattachGrace', 'settings.lossThreshold', 'settings.probation'] as const)
      expect(screen.getByRole('button', { name: t('metrics.whatIs', { name: t(key) }) })).toBeTruthy()
  })

  describe('history and automatic tests', () => {
    const saved = () => vi.mocked(api.saveSettings).mock.calls[0][0]
    const autoSwitch = () => screen.getByRole('switch', { name: t('settings.autoTest.name') })
    const every = (n: number) =>
      screen.getByLabelText(
        t(n === 1 ? 'settings.autoTest.everyOne' : n === 2 ? 'settings.autoTest.everyTwo' : 'settings.autoTest.everyFew', { n }),
      )
    const radio = (label: string, group: string) =>
      screen.getAllByLabelText(label).find((el) => (el as HTMLInputElement).name === group) as HTMLInputElement
    const saveButton = () => screen.getByRole('button', { name: t('settings.saveChanges') }) as HTMLButtonElement

    it('has automatic tests off, with no schedule shown', () => {
      render(Settings)
      expect(autoSwitch().getAttribute('aria-checked')).toBe('false')
      expect(screen.queryByLabelText(t('settings.autoTest.everyFew', { n: 3 }))).toBeNull()
    })

    it('turns them on at every 3 h, and a preset sets the interval', async () => {
      render(Settings)
      await fireEvent.click(autoSwitch())
      expect((every(3) as HTMLInputElement).checked).toBe(true)
      await fireEvent.click(every(1))
      await fireEvent.click(saveButton())
      await waitFor(() => expect(api.saveSettings).toHaveBeenCalledOnce())
      expect(saved().history.autoSpeedTest).toMatchObject({ enabled: true, everyH: 1 })
    })

    it('checks a custom interval before saving it', async () => {
      render(Settings)
      await fireEvent.click(autoSwitch())
      await fireEvent.click(radio(t('settings.autoTest.custom'), 'auto-every'))
      const input = screen.getByLabelText(t('settings.autoTest.customHours')) as HTMLInputElement
      await fireEvent.input(input, { target: { value: '200' } })
      expect(screen.getByRole('alert')).toBeTruthy()
      expect(saveButton().disabled).toBe(true)
      await fireEvent.input(input, { target: { value: '12' } })
      expect(screen.queryByRole('alert')).toBeNull()
      await fireEvent.click(saveButton())
      await waitFor(() => expect(api.saveSettings).toHaveBeenCalledOnce())
      expect(saved().history.autoSpeedTest.everyH).toBe(12)
    })

    it('warns when the tests would use more than 300 MB a day', async () => {
      render(Settings)
      await fireEvent.click(autoSwitch())
      await fireEvent.click(every(5))
      // 24 / 5 tests of about 40 MB.
      expect(screen.getByTestId('auto-data-use').textContent).toContain('192')
      expect(screen.queryByTestId('auto-data-warning')).toBeNull()
      await fireEvent.click(every(1))
      expect(screen.getByTestId('auto-data-warning').textContent).toContain('960')
      expect(screen.getByText(t('settings.autoTest.dataUseHigh'))).toBeTruthy()
    })

    it('keeps Ookla off limits until its terms are accepted', async () => {
      render(Settings)
      await fireEvent.click(autoSwitch())
      const ookla = screen.getByLabelText(t('speedtest.settings.ookla')) as HTMLInputElement
      expect(ookla.disabled).toBe(true)
      expect(screen.getByText(t('settings.autoTest.ooklaTerms'))).toBeTruthy()
      cleanup()
      app.settings = { ...structuredClone(base), speedTest: { ...base.speedTest, ooklaTermsAccepted: true } }
      render(Settings)
      await fireEvent.click(autoSwitch())
      expect((screen.getByLabelText(t('speedtest.settings.ookla')) as HTMLInputElement).disabled).toBe(false)
      expect(screen.queryByText(t('settings.autoTest.ooklaTerms'))).toBeNull()
    })

    it('shows when the next test is due', async () => {
      app.settings = structuredClone(base)
      app.settings.history.autoSpeedTest.enabled = true
      const next = new Date()
      next.setHours(23, 59, 0, 0)
      vi.mocked(api.autoSpeedTestStatus).mockResolvedValue(autoStatus({ enabled: true, nextSlotMs: next.getTime() }))
      render(Settings)
      await waitFor(() => expect(screen.getByTestId('auto-status').textContent).toContain('23:59'))
    })

    it('keeps 90 days by default, and Forever saves no limit', async () => {
      render(Settings)
      const ninety = screen.getByLabelText(
        t('settings.history.recommended', { label: t('settings.history.days', { n: 90 }) }),
      ) as HTMLInputElement
      expect(ninety.checked).toBe(true)
      await fireEvent.click(screen.getByLabelText(t('settings.history.forever')))
      await fireEvent.click(saveButton())
      await waitFor(() => expect(api.saveSettings).toHaveBeenCalledOnce())
      expect(saved().history.retentionDays).toBeNull()
    })

    it('checks custom days', async () => {
      render(Settings)
      await fireEvent.click(radio(t('settings.history.custom'), 'keep'))
      const input = screen.getByLabelText(t('settings.history.customDays')) as HTMLInputElement
      expect(input.value).toBe('90')
      await fireEvent.input(input, { target: { value: '3' } })
      expect(saveButton().disabled).toBe(true)
      await fireEvent.input(input, { target: { value: '45' } })
      await fireEvent.click(saveButton())
      await waitFor(() => expect(api.saveSettings).toHaveBeenCalledOnce())
      expect(saved().history.retentionDays).toBe(45)
    })

    it('shows the disk use, and clears only after a confirm, once', async () => {
      render(Settings)
      await waitFor(() => expect(screen.getByTestId('history-disk').textContent).toContain('2.5'))
      await fireEvent.click(screen.getByRole('button', { name: t('settings.history.clear') }))
      expect(api.clearHistory).not.toHaveBeenCalled()
      const dialog = screen.getByRole('alertdialog')
      expect(dialog.textContent).toContain(t('settings.history.clearBody'))
      await fireEvent.click(screen.getByRole('button', { name: t('settings.history.clearConfirm') }))
      await waitFor(() => expect(api.clearHistory).toHaveBeenCalledOnce())
      await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull())
      expect(app.toasts.map((x) => x.text)).toContain(t('settings.history.cleared'))
    })

    it('shows a small history in KB, not as 0.0 MB', async () => {
      vi.mocked(api.historyOverview).mockResolvedValue(overviewFixture({ dbBytes: 40_960 }))
      render(Settings)
      await waitFor(() => expect(screen.getByTestId('history-disk').textContent).toContain(t('scan.kb', { n: '41' })))
    })

    it('says to wait when the history is busy', async () => {
      vi.mocked(api.clearHistory).mockRejectedValue('wait for the auto-scan or speed test to finish')
      render(Settings)
      await fireEvent.click(screen.getByRole('button', { name: t('settings.history.clear') }))
      await fireEvent.click(screen.getByRole('button', { name: t('settings.history.clearConfirm') }))
      await waitFor(() => expect(screen.getByText(t('errors.historyBusy'))).toBeTruthy())
      expect(api.clearHistory).toHaveBeenCalledOnce()
    })
  })
})
