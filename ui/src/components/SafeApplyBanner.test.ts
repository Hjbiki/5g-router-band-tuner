import { tick } from 'svelte'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/svelte'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))
vi.mock('../lib/backend', () => ({ api: { keepNow: vi.fn(), revertNow: vi.fn() } }))

import { api } from '../lib/backend'
import { app } from '../lib/store.svelte'
import { history, resetHistory } from '../lib/history.svelte'
import { t } from '../lib/i18n'
import SafeApplyBanner from './SafeApplyBanner.svelte'
import { settingsFixture, snap } from '../test-fixtures'
import type { Phase } from '../lib/bindings/Phase'
import { formatNumber, isolate } from '../lib/format'
import { bandList } from '../lib/eventText'

const show = (phase: Phase, session: 'loggedIn' | 'takenOver' = 'loggedIn') => {
  app.snapshot = snap({
    session: { kind: session },
    safeApply: { phase, secondsLeft: 42, previous: null, requested: null },
  })
}

describe('SafeApplyBanner', () => {
  afterEach(() => {
    cleanup()
    vi.clearAllMocks()
  })

  it('renders nothing when idle', () => {
    show({ kind: 'idle' })
    const { container } = render(SafeApplyBanner)
    expect(container.querySelector('section')).toBeNull()
  })

  it('Keep only in probation; Revert disabled while applying', async () => {
    show({ kind: 'applying' })
    render(SafeApplyBanner)
    expect(screen.queryByRole('button', { name: t('safeApply.keep') })).toBeNull()
    expect((screen.getByRole('button', { name: t('safeApply.revert') }) as HTMLButtonElement).disabled).toBe(true)
    cleanup()
    show({ kind: 'probation' })
    render(SafeApplyBanner)
    await fireEvent.click(screen.getByRole('button', { name: t('safeApply.keep') }))
    expect(api.keepNow).toHaveBeenCalled()
    // A second command can't be queued behind the first.
    await fireEvent.click(screen.getByRole('button', { name: t('safeApply.revert') }))
    expect(api.revertNow).not.toHaveBeenCalled()
    cleanup()
    show({ kind: 'probation' })
    render(SafeApplyBanner)
    await fireEvent.click(screen.getByRole('button', { name: t('safeApply.revert') }))
    expect(api.revertNow).toHaveBeenCalled()
  })

  it('Keep responds at once: both buttons disabled and "Keeping…" until the phase changes', async () => {
    show({ kind: 'probation' })
    render(SafeApplyBanner)
    await fireEvent.click(screen.getByRole('button', { name: t('safeApply.keep') }))
    const keeping = screen.getByRole('button', { name: t('safeApply.keeping') }) as HTMLButtonElement
    expect(keeping.disabled).toBe(true)
    expect((screen.getByRole('button', { name: t('safeApply.revert') }) as HTMLButtonElement).disabled).toBe(true)
    await fireEvent.click(keeping)
    expect(api.keepNow).toHaveBeenCalledTimes(1)

    show({ kind: 'committed' })
    await tick()
    expect(screen.queryByRole('button', { name: t('safeApply.keeping') })).toBeNull()
  })

  it('Revert shows "Reverting…"; a failed command brings the buttons back', async () => {
    vi.mocked(api.revertNow).mockRejectedValueOnce('router service is not running')
    show({ kind: 'probation' })
    render(SafeApplyBanner)
    await fireEvent.click(screen.getByRole('button', { name: t('safeApply.revert') }))
    await tick()
    const revert = screen.getByRole('button', { name: t('safeApply.revert') }) as HTMLButtonElement
    expect(revert.disabled).toBe(false)

    vi.mocked(api.revertNow).mockResolvedValueOnce(undefined)
    await fireEvent.click(revert)
    expect((screen.getByRole('button', { name: t('safeApply.revertingNow') }) as HTMLButtonElement).disabled).toBe(true)
  })

  it('shows the urgent close-web-UI text in revertPending when taken over', () => {
    show({ kind: 'revertPending', reason: 'lossOfService' }, 'takenOver')
    render(SafeApplyBanner)
    expect(screen.getByText(t('safeApply.closeWebUi'))).toBeTruthy()
    expect(screen.getByText(t('safeApply.reason.connectionLost'))).toBeTruthy()
  })

  it('shows a repeated identical revert again after dismissal', async () => {
    show({ kind: 'reverted', reason: 'noReattach', fallback: false })
    render(SafeApplyBanner)
    await fireEvent.click(screen.getByRole('button', { name: t('safeApply.dismiss') }))
    expect(screen.queryByText(t('safeApply.reason.noReattach'))).toBeNull()
    show({ kind: 'probation' })
    await tick()
    show({ kind: 'reverted', reason: 'noReattach', fallback: false })
    await tick()
    expect(screen.getByText(t('safeApply.reason.noReattach'))).toBeTruthy()
  })

  it('shows the reason after a revert', () => {
    show({ kind: 'reverted', reason: 'noReattach', fallback: false })
    render(SafeApplyBanner)
    expect(screen.getByText(t('safeApply.reason.noReattach'))).toBeTruthy()
  })

  it('numbers revert attempts from 1', () => {
    show({ kind: 'reverting', attempt: 0, fallback: false })
    render(SafeApplyBanner)
    expect(screen.getByText(t('safeApply.reverting', { attempt: isolate(formatNumber(1)) }))).toBeTruthy()
    cleanup()
    show({ kind: 'reverting', attempt: 2, fallback: false })
    render(SafeApplyBanner)
    expect(screen.getByText(t('safeApply.reverting', { attempt: isolate(formatNumber(3)) }))).toBeTruthy()
  })

  it('offers Revert now after a failed revert', async () => {
    show({ kind: 'revertFailed' })
    render(SafeApplyBanner)
    const revert = screen.getByRole('button', { name: t('safeApply.revert') }) as HTMLButtonElement
    expect(revert.disabled).toBe(false)
    await fireEvent.click(revert)
    expect(api.revertNow).toHaveBeenCalled()
  })

  it('shows the bands, the countdown and the track while reconnecting', () => {
    app.snapshot = snap({
      safeApply: {
        phase: { kind: 'reattachGrace' },
        secondsLeft: 52,
        previous: { ...snap().config!, lteBands: '5' },
        requested: { ...snap().config!, lteBands: '4' },
      },
    })
    render(SafeApplyBanner)
    const banner = screen.getByRole('status')
    expect(banner.textContent).toContain('B1, B3')
    expect(banner.querySelector('.route')?.textContent?.trim().endsWith('B3')).toBe(true)
    expect(screen.getByText('0:52')).toBeTruthy()
    const track = screen.getByRole('list', { name: t('safeApply.trackLabel') })
    const steps = track.querySelectorAll('li')
    expect(steps).toHaveLength(6)
    expect(steps[3].getAttribute('aria-current')).toBe('step')
    expect(steps[3].textContent?.trim()).toBe(t('safeApply.step.reconnecting'))
    // Keep is visible but waits for the router.
    expect((screen.getByRole('button', { name: t('safeApply.keep') }) as HTMLButtonElement).disabled).toBe(true)
    expect((screen.getByRole('button', { name: t('safeApply.revert') }) as HTMLButtonElement).disabled).toBe(false)
  })

  it('marks reconnect done and the trial current in probation', () => {
    show({ kind: 'probation' })
    render(SafeApplyBanner)
    const steps = screen.getByRole('list', { name: t('safeApply.trackLabel') }).querySelectorAll('li')
    expect(steps[3].textContent?.trim()).toBe(t('safeApply.step.reconnected'))
    expect(steps[4].getAttribute('aria-current')).toBe('step')
  })

  it('names the kept bands when the change is done', () => {
    app.snapshot = snap({
      safeApply: { phase: { kind: 'committed' }, secondsLeft: null, previous: null, requested: { ...snap().config!, lteBands: '4' } },
    })
    render(SafeApplyBanner)
    expect(screen.getByRole('status').textContent).toContain('Kept B3.')
    expect(screen.getByRole('button', { name: t('safeApply.dismiss') })).toBeTruthy()
  })

  it('shows a failed revert as an alert with the router address', () => {
    app.settings = settingsFixture()
    show({ kind: 'revertFailed' })
    render(SafeApplyBanner)
    const alert = screen.getByRole('alert')
    expect(alert.textContent).toContain(t('safeApply.revertFailedTitle'))
    expect(alert.textContent).toContain('192.168.8.1')
    app.settings = null
  })

  it('says it switched to Auto when the revert ended on the fallback', () => {
    show({ kind: 'reverted', reason: 'noReattach', fallback: true })
    render(SafeApplyBanner)
    expect(screen.getByText(t('safeApply.revertedToAuto'))).toBeTruthy()
    expect(screen.queryByText(t('safeApply.reverted'))).toBeNull()
  })

  it('shows the step of a two-step apply', () => {
    app.settings = settingsFixture()
    app.snapshot = snap({
      safeApply: {
        phase: { kind: 'probation' },
        secondsLeft: 42,
        previous: null,
        requested: null,
        sequence: { step: 1, total: 2, primary: 3, finalConfig: snap().config!, origin: 'user' },
        policy: { revertOnLoss: true, lossIgnored: false },
      },
    })
    render(SafeApplyBanner)
    expect(screen.getByText(t('safeApply.sequenceStep1', { band: isolate('B3') }))).toBeTruthy()
    expect(screen.queryByText(t('safeApply.lossIgnored'))).toBeNull()
  })

  it('names the steps of a band switch for a new IP by their band sets', () => {
    app.settings = settingsFixture()
    const original = { ...snap().config!, lteBands: '5' }
    const step1 = { ...snap().config!, lteBands: '4' }
    const view = (step: number, kind: 'probation' | 'committed') =>
      snap({
        safeApply: {
          phase: { kind },
          secondsLeft: kind === 'probation' ? 42 : null,
          previous: null,
          requested: null,
          sequence: { step, total: 2, primary: 3, finalConfig: original, origin: 'ipReattach', step1 },
          policy: { revertOnLoss: true, lossIgnored: false },
        },
      })
    app.snapshot = view(1, 'probation')
    render(SafeApplyBanner)
    expect(screen.getByText(t('safeApply.reattachStep1', { bands: bandList([3], 'western') }))).toBeTruthy()
    expect(screen.queryByText(t('safeApply.sequenceStep1', { band: isolate('B3') }))).toBeNull()
    cleanup()
    app.snapshot = view(1, 'committed')
    render(SafeApplyBanner)
    expect(screen.getByText(t('safeApply.reattachWaiting', { bands: bandList([1, 3], 'western') }))).toBeTruthy()
    expect(screen.queryByRole('button', { name: t('safeApply.dismiss') })).toBeNull()
    cleanup()
    app.snapshot = view(2, 'probation')
    render(SafeApplyBanner)
    expect(screen.getByText(t('safeApply.reattachStep2', { bands: bandList([1, 3], 'western') }))).toBeTruthy()
  })

  it('between the two steps it waits for the other bands and cannot be dismissed', () => {
    app.settings = settingsFixture()
    app.snapshot = snap({
      safeApply: {
        phase: { kind: 'committed' },
        secondsLeft: null,
        previous: null,
        requested: { ...snap().config!, lteBands: '4' },
        sequence: { step: 1, total: 2, primary: 3, finalConfig: snap().config!, origin: 'user' },
        policy: { revertOnLoss: true, lossIgnored: false },
      },
    })
    render(SafeApplyBanner)
    expect(screen.getByText(t('safeApply.sequenceWaiting', { band: isolate('B3') }))).toBeTruthy()
    expect(screen.queryByText(/The change is done/)).toBeNull()
    expect(screen.queryByRole('button', { name: t('safeApply.dismiss') })).toBeNull()
  })

  it('after the last step it is done and can be dismissed', () => {
    app.settings = settingsFixture()
    app.snapshot = snap({
      safeApply: { phase: { kind: 'committed' }, secondsLeft: null, previous: null, requested: null, sequence: null },
    })
    render(SafeApplyBanner)
    expect(screen.getByText(t('safeApply.committed'))).toBeTruthy()
    expect(screen.getByRole('button', { name: t('safeApply.dismiss') })).toBeTruthy()
  })

  it('says plainly when a lost connection is not undone', () => {
    app.settings = settingsFixture()
    app.snapshot = snap({
      safeApply: {
        phase: { kind: 'probation' },
        secondsLeft: 42,
        previous: null,
        requested: null,
        sequence: null,
        policy: { revertOnLoss: false, lossIgnored: true },
      },
    })
    render(SafeApplyBanner)
    expect(screen.getByText(t('safeApply.lossIgnored'))).toBeTruthy()
  })

  it('links to the event log once a change is undone, or failed to undo', async () => {
    resetHistory()
    history.range = { kind: '7d' }
    for (const phase of [{ kind: 'committed' }, { kind: 'probation' }] as Phase[]) {
      show(phase)
      render(SafeApplyBanner)
      expect(screen.queryByRole('button', { name: t('safeApply.seeWhatHappened') })).toBeNull()
      cleanup()
    }
    show({ kind: 'revertFailed' })
    render(SafeApplyBanner)
    expect(screen.getByRole('button', { name: t('safeApply.seeWhatHappened') })).toBeTruthy()
    cleanup()

    show({ kind: 'reverted', reason: 'lossOfService', fallback: false })
    render(SafeApplyBanner)
    await fireEvent.click(screen.getByRole('button', { name: t('safeApply.seeWhatHappened') }))
    expect(app.tab).toBe('history')
    expect(app.historyFocus).toBe('latestChange')
    expect(history.range).toEqual({ kind: '24h' })
    expect(api.revertNow).not.toHaveBeenCalled()
    app.tab = 'dashboard'
    app.historyFocus = null
    resetHistory()
  })

  describe('for an APN switch', () => {
    const fp = (n: number) => String(n).repeat(16).slice(0, 16)
    const profile = (index: number, name: string) => ({
      index,
      name,
      apn: null,
      apnStatic: false,
      username: '',
      hasPassword: false,
      auth: 'auto' as const,
      ipType: 'ipv4v6' as const,
      readOnly: false,
      valid: true,
      fingerprint: fp(index),
    })
    const showApn = (phase: Phase) => {
      app.snapshot = snap({
        apn: { current: 1, autoApn: false, profiles: [profile(1, 'Operator internet'), profile(2, 'Work')] },
        apnApply: {
          phase,
          secondsLeft: 30,
          previous: { index: 1, fingerprint: fp(1) },
          requested: { index: 2, fingerprint: fp(2) },
        },
      })
    }

    it('names the profiles, not bands, and has its own title', () => {
      app.settings = settingsFixture()
      showApn({ kind: 'reattachGrace' })
      render(SafeApplyBanner, { subject: 'apn' })
      const banner = screen.getByRole('status', { name: t('safeApply.apn.title') })
      expect(banner.getAttribute('data-subject')).toBe('apn')
      expect(screen.getByTestId('apn-route').textContent).toContain('Operator internet')
      expect(screen.getByTestId('apn-route').textContent).toContain('Work')
      expect(banner.textContent).not.toContain('B1')
      cleanup()
      showApn({ kind: 'applying' })
      render(SafeApplyBanner, { subject: 'apn' })
      expect(screen.getByText(t('safeApply.apn.applying'))).toBeTruthy()
    })

    it('Keep and Revert work the same way', async () => {
      app.settings = settingsFixture()
      showApn({ kind: 'probation' })
      render(SafeApplyBanner, { subject: 'apn' })
      await fireEvent.click(screen.getByRole('button', { name: t('safeApply.keep') }))
      expect(api.keepNow).toHaveBeenCalled()
    })

    it('says plainly when the old APN could not be restored, with no band link', () => {
      app.settings = settingsFixture()
      showApn({ kind: 'revertFailed' })
      render(SafeApplyBanner, { subject: 'apn' })
      expect(screen.getByRole('alert').textContent).toContain(t('safeApply.apn.revertFailedTitle'))
      expect(screen.queryByRole('button', { name: t('safeApply.seeWhatHappened') })).toBeNull()
    })

    it('each banner shows only its own change', () => {
      app.settings = settingsFixture()
      showApn({ kind: 'probation' })
      const band = render(SafeApplyBanner)
      expect(band.container.querySelector('section')).toBeNull()
      cleanup()
      show({ kind: 'probation' })
      const apn = render(SafeApplyBanner, { subject: 'apn' })
      expect(apn.container.querySelector('section')).toBeNull()
    })
  })
})
