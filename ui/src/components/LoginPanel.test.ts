import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))
vi.mock('../lib/backend', () => ({
  api: {
    saveCredentials: vi.fn(),
    forgetCredentials: vi.fn(),
    connect: vi.fn(),
    connectTrusting: vi.fn(),
    credentialStatus: vi.fn(),
    logout: vi.fn(),
  },
}))

import { api } from '../lib/backend'
import { app } from '../lib/store.svelte'
import { t, type MessageKey } from '../lib/i18n'
import { credentials } from '../lib/credentials.svelte'
import { scan } from '../lib/scan.svelte'
import { resetSpeedTest, speed } from '../lib/speedtest.svelte'
import LoginPanel, { resetLoginPanel } from './LoginPanel.svelte'
import { settingsFixture, snap } from '../test-fixtures'
import type { SessionView } from '../lib/bindings/SessionView'

const NONE = { saved: false, rejected: false }
const SAVED = { saved: true, rejected: false }

function session(view: SessionView) {
  app.snapshot = snap({ session: view })
}
const pwInput = () => screen.getByLabelText(t('settings.password')) as HTMLInputElement
const saveConnect = () => screen.getByRole('button', { name: t('login.saveConnect') }) as HTMLButtonElement

describe('LoginPanel', () => {
  beforeEach(() => {
    app.settings = settingsFixture({ onboardingDone: true })
    app.tab = 'dashboard'
    credentials.status = NONE
    session({ kind: 'notConfigured' })
    resetLoginPanel()
    vi.mocked(api.saveCredentials).mockResolvedValue(undefined)
    vi.mocked(api.forgetCredentials).mockResolvedValue(undefined)
    vi.mocked(api.connect).mockResolvedValue(undefined)
    vi.mocked(api.connectTrusting).mockResolvedValue(undefined)
    vi.mocked(api.credentialStatus).mockResolvedValue(SAVED)
    vi.mocked(api.logout).mockResolvedValue({ confirmed: true })
    app.toasts.splice(0)
  })
  afterEach(() => {
    cleanup()
    vi.useRealTimers()
    vi.clearAllMocks()
  })

  it('never pre-fills the password, saves it once before connecting, and clears it', async () => {
    const order: string[] = []
    vi.mocked(api.saveCredentials).mockImplementation(async () => void order.push('save'))
    vi.mocked(api.credentialStatus).mockImplementation(async () => (order.push('status'), SAVED))
    vi.mocked(api.connect).mockImplementation(async () => void order.push('connect'))
    render(LoginPanel, { variant: 'settings' })
    expect(screen.getByText(t('login.title.needPassword'))).toBeTruthy()
    const pw = pwInput()
    expect(pw.value).toBe('')
    expect(pw.type).toBe('password')
    expect(saveConnect().disabled).toBe(true)
    await fireEvent.input(pw, { target: { value: 'secret-pw' } })
    await fireEvent.click(saveConnect())
    await waitFor(() => expect(api.connect).toHaveBeenCalledOnce())
    expect(api.saveCredentials).toHaveBeenCalledExactlyOnceWith('secret-pw')
    expect(order).toEqual(['save', 'status', 'connect'])
    expect(pw.value).toBe('')
    expect(screen.getByText(t('login.checking'))).toBeTruthy()
  })

  it('stops checking once the login has been answered', async () => {
    render(LoginPanel, { variant: 'settings' })
    await fireEvent.input(pwInput(), { target: { value: 'pw' } })
    await fireEvent.click(saveConnect())
    await waitFor(() => expect(api.connect).toHaveBeenCalled())
    session({ kind: 'connecting' })
    await waitFor(() => expect(screen.getByText(t('login.checking'))).toBeTruthy())
    session({ kind: 'badCredentials' })
    await waitFor(() => expect(screen.queryByText(t('login.checking'))).toBeNull())
    expect(screen.getByText(t('login.title.wrong'))).toBeTruthy()
  })

  it('offers no way to resend a rejected password', async () => {
    for (const view of [{ kind: 'badCredentials' }, { kind: 'loginRejected', code: 108009 }] as SessionView[]) {
      session(view)
      credentials.status = { saved: true, rejected: true }
      render(LoginPanel, { variant: 'banner' })
      expect(screen.getByText(t('login.title.wrong'))).toBeTruthy()
      // Only Save and connect (disabled while empty), the eye toggle and the help button.
      for (const b of screen.getAllByRole('button')) await fireEvent.click(b)
      expect(api.connect).not.toHaveBeenCalled()
      expect(api.connectTrusting).not.toHaveBeenCalled()
      expect(api.saveCredentials).not.toHaveBeenCalled()
      cleanup()
    }
  })

  it('shows a rejected password from an earlier run as wrong, not as "not set up"', () => {
    credentials.status = { saved: true, rejected: true }
    render(LoginPanel, { variant: 'banner' })
    expect(screen.getByText(t('login.title.wrong'))).toBeTruthy()
    expect(document.activeElement).toBe(pwInput())
  })

  it('keeps the form disabled until the lockout countdown ends', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'setInterval', 'clearTimeout', 'clearInterval', 'Date'] })
    session({ kind: 'lockedOut', waitS: 3 })
    render(LoginPanel, { variant: 'settings' })
    expect(screen.getByText(t('login.body.locked', { time: '⁦0:03⁩' }))).toBeTruthy()
    expect(pwInput().disabled).toBe(true)
    expect(saveConnect().disabled).toBe(true)
    await vi.advanceTimersByTimeAsync(3100)
    expect(screen.getByText(t('login.body.lockedOver'))).toBeTruthy()
    expect(pwInput().disabled).toBe(false)
    await fireEvent.input(pwInput(), { target: { value: 'pw' } })
    expect(saveConnect().disabled).toBe(false)
    expect(api.connect).not.toHaveBeenCalled()
  })

  it('asks about the router: Yes logs in trusting it, Not now hides only the banner', async () => {
    session({ kind: 'confirmRouter', firstTime: true })
    credentials.status = SAVED
    render(LoginPanel, { variant: 'banner' })
    expect(screen.getByText(t('login.body.confirmNew', { host: '⁦192.168.8.1⁩' }))).toBeTruthy()
    await fireEvent.click(screen.getByRole('button', { name: t('login.confirmYes') }))
    expect(api.connectTrusting).toHaveBeenCalledOnce()
    expect(api.connect).not.toHaveBeenCalled()

    await fireEvent.click(screen.getByRole('button', { name: t('login.notNow') }))
    expect(screen.queryByTestId('login-panel')).toBeNull()
    cleanup()
    // Settings still asks.
    render(LoginPanel, { variant: 'settings' })
    expect(screen.getByRole('button', { name: t('login.confirmYes') })).toBeTruthy()
    expect(screen.queryByRole('button', { name: t('login.notNow') })).toBeNull()
  })

  it('asks before removing the saved password', async () => {
    session({ kind: 'loggedIn' })
    credentials.status = SAVED
    vi.mocked(api.credentialStatus).mockResolvedValue(NONE)
    render(LoginPanel, { variant: 'settings' })
    expect(screen.getByText(t('login.savedFor', { account: '⁦192.168.8.1/admin⁩' }))).toBeTruthy()
    await fireEvent.click(screen.getByRole('button', { name: t('login.remove') }))
    expect(api.forgetCredentials).not.toHaveBeenCalled()
    expect(screen.getByRole('alertdialog').textContent).toContain(t('login.removeConfirm'))
    await fireEvent.click(screen.getByRole('button', { name: t('app.cancel') }))
    expect(screen.queryByRole('alertdialog')).toBeNull()
    expect(api.forgetCredentials).not.toHaveBeenCalled()

    await fireEvent.click(screen.getByRole('button', { name: t('login.remove') }))
    await fireEvent.click(screen.getByRole('button', { name: t('login.removeYes') }))
    await waitFor(() => expect(api.credentialStatus).toHaveBeenCalled())
    expect(api.forgetCredentials).toHaveBeenCalledOnce()
    await waitFor(() => expect(credentials.status).toEqual(NONE))
  })

  it('the eye button shows and hides the password', async () => {
    render(LoginPanel, { variant: 'settings' })
    const eye = screen.getByRole('button', { name: t('login.show') })
    expect(eye.getAttribute('aria-pressed')).toBe('false')
    await fireEvent.click(eye)
    expect(pwInput().type).toBe('text')
    const hide = screen.getByRole('button', { name: t('login.hide') })
    expect(hide.getAttribute('aria-pressed')).toBe('true')
    await fireEvent.click(hide)
    expect(pwInput().type).toBe('password')
  })

  it('waits for an unsaved address or username before taking a password', async () => {
    render(LoginPanel, { variant: 'settings', accountDirty: true })
    expect(pwInput().disabled).toBe(true)
    expect(screen.getByTestId('login-account-dirty').textContent).toBe(t('login.accountDirty'))
    expect(saveConnect().disabled).toBe(true)
  })

  it('the banner shows nothing while connected or connecting', () => {
    session({ kind: 'loggedIn' })
    render(LoginPanel, { variant: 'banner' })
    expect(screen.queryByTestId('login-panel')).toBeNull()
    cleanup()
    session({ kind: 'connecting' })
    render(LoginPanel, { variant: 'banner' })
    expect(screen.queryByTestId('login-panel')).toBeNull()
  })

  it('a saved password that is not logged in offers Connect, after the launch connect had its chance', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'setInterval', 'clearTimeout', 'clearInterval', 'Date'] })
    credentials.status = SAVED
    render(LoginPanel, { variant: 'banner' })
    expect(screen.queryByTestId('login-panel')).toBeNull()
    await vi.advanceTimersByTimeAsync(2100)
    await fireEvent.click(screen.getByRole('button', { name: t('settings.connect') }))
    expect(api.connect).toHaveBeenCalledOnce()
  })

  it('a refused token says it is safe to try again', async () => {
    session({ kind: 'loginRejected', code: 125003 })
    credentials.status = SAVED
    render(LoginPanel, { variant: 'settings' })
    expect(screen.getByText(t('login.body.token'))).toBeTruthy()
    await fireEvent.click(screen.getByRole('button', { name: t('login.tryAgain') }))
    expect(api.connect).toHaveBeenCalledOnce()
  })

  it('needs no password in demo mode', () => {
    app.settings = settingsFixture({ driver: 'mock', onboardingDone: true })
    credentials.status = SAVED
    session({ kind: 'loggedIn' })
    render(LoginPanel, { variant: 'settings' })
    expect(screen.getByText(t('login.demo'))).toBeTruthy()
    expect(screen.queryByLabelText(t('settings.password'))).toBeNull()
    expect(screen.queryByRole('button', { name: t('login.change') })).toBeNull()
  })

  it('a new password typed while logged in is checked, not taken from the old session', async () => {
    session({ kind: 'loggedIn' })
    credentials.status = SAVED
    render(LoginPanel, { variant: 'settings' })
    await fireEvent.click(screen.getByRole('button', { name: t('login.change') }))
    await fireEvent.input(pwInput(), { target: { value: 'new-pw' } })
    await fireEvent.click(saveConnect())
    await waitFor(() => expect(api.connect).toHaveBeenCalledOnce())
    // Still the old session: no answer yet.
    expect(screen.getByText(t('login.checking'))).toBeTruthy()
    session({ kind: 'connecting' })
    await waitFor(() => expect(screen.getByText(t('login.checking'))).toBeTruthy())
    session({ kind: 'loggedIn' })
    await waitFor(() => expect(screen.queryByText(t('login.checking'))).toBeNull())
  })

  it('the banner stays out of the way on Settings, which has its own panel', () => {
    app.tab = 'settings'
    render(LoginPanel, { variant: 'banner' })
    expect(screen.queryByTestId('login-panel')).toBeNull()
    cleanup()
    render(LoginPanel, { variant: 'settings' })
    expect(screen.getAllByLabelText(t('settings.password'))).toHaveLength(1)
  })

  it('the banner hides behind the first-run guide and never takes its focus', () => {
    app.settings = settingsFixture({ onboardingDone: false })
    session({ kind: 'badCredentials' })
    const outside = document.createElement('button')
    document.body.append(outside)
    outside.focus()
    render(LoginPanel, { variant: 'banner' })
    expect(screen.queryByTestId('login-panel')).toBeNull()
    expect(document.activeElement).toBe(outside)
    outside.remove()
  })

  describe('Log out', () => {
    const logoutButton = () => screen.getByRole('button', { name: t('login.logout') }) as HTMLButtonElement

    it('sits before Reconnect, logs out in one click and says so', async () => {
      session({ kind: 'loggedIn' })
      credentials.status = SAVED
      render(LoginPanel, { variant: 'settings' })
      const names = screen.getAllByRole('button').map((b) => b.textContent?.trim())
      expect(names.indexOf(t('login.logout'))).toBe(names.indexOf(t('settings.reconnect')) - 1)
      await fireEvent.click(logoutButton())
      expect(api.logout).toHaveBeenCalledOnce()
      expect(screen.queryByRole('alertdialog')).toBeNull()
      await waitFor(() => expect(app.toasts.map((x) => x.text)).toEqual([t('login.loggedOutToast')]))
    })

    it('adds a soft note when the router did not confirm', async () => {
      vi.mocked(api.logout).mockResolvedValue({ confirmed: false })
      session({ kind: 'loggedIn' })
      render(LoginPanel, { variant: 'settings' })
      await fireEvent.click(logoutButton())
      await waitFor(() =>
        expect(app.toasts.map((x) => x.text)).toEqual([
          t('login.loggedOutToast'),
          t('login.logoutUnconfirmed', { minutes: '\u2066' + '5' + '\u2069' }),
        ]),
      )
      expect(app.toasts.every((x) => x.kind === 'info')).toBe(true)
    })

    it('waits, and says why, while a band change, an IP refresh, an APN change, an auto-scan or a speed test runs', async () => {
      const blocks: [() => void, () => void, MessageKey][] = [
        [
          () =>
            (app.snapshot = snap({
              safeApply: { phase: { kind: 'probation' }, secondsLeft: 100, previous: null, requested: null },
            })),
          () => {},
          'login.logoutBlocked',
        ],
        [
          () => (app.snapshot = snap({ ipRefresh: { active: true, phase: 'off' } as never })),
          () => {},
          'login.logoutBlockedIpRefresh',
        ],
        [
          () =>
            (app.snapshot = snap({
              apnApply: { phase: { kind: 'probation' }, secondsLeft: 100, previous: null, requested: null },
            })),
          () => {},
          'login.logoutBlockedApn',
        ],
        [() => (scan.file = { status: 'running' } as never), () => (scan.file = null), 'login.logoutBlockedScan'],
        [() => (speed.running = true), () => resetSpeedTest(), 'login.logoutBlockedTest'],
      ]
      for (const [set, unset, key] of blocks) {
        session({ kind: 'loggedIn' })
        set()
        render(LoginPanel, { variant: 'settings' })
        const b = logoutButton()
        expect(b.disabled, key).toBe(true)
        const note = document.getElementById(b.getAttribute('aria-describedby') ?? '')
        expect(note?.textContent).toBe(t(key))
        await fireEvent.click(b)
        expect(api.logout).not.toHaveBeenCalled()
        unset()
        cleanup()
      }
      session({ kind: 'loggedIn' })
      render(LoginPanel, { variant: 'settings' })
      expect(logoutButton().disabled).toBe(false)
      expect(logoutButton().getAttribute('aria-describedby')).toBeNull()
    })

    it('shows the refusal if the backend still refuses', async () => {
      vi.mocked(api.logout).mockRejectedValue({ kind: 'validation', message: 'a speed test is running' })
      session({ kind: 'loggedIn' })
      render(LoginPanel, { variant: 'settings' })
      await fireEvent.click(logoutButton())
      await waitFor(() => expect(screen.getByRole('alert').textContent).toBe(t('danger.blockedSpeed')))
      expect(app.toasts).toEqual([])
    })

    it('logged out: Connect comes first, and the password can still be changed or removed', async () => {
      session({ kind: 'loggedOut' })
      credentials.status = SAVED
      render(LoginPanel, { variant: 'settings' })
      const panel = screen.getByTestId('login-panel')
      expect(panel.dataset.state).toBe('loggedOut')
      expect(panel.classList.contains('neutral')).toBe(true)
      expect(panel.textContent).toContain(t('login.title.loggedOut'))
      expect(panel.textContent).toContain(t('login.body.loggedOut'))
      const connect = screen.getByRole('button', { name: t('settings.connect') })
      expect(connect.classList.contains('primary')).toBe(true)
      expect(screen.getByRole('button', { name: t('login.change') })).toBeTruthy()
      expect(screen.getByRole('button', { name: t('login.remove') })).toBeTruthy()
      expect(screen.queryByRole('button', { name: t('login.logout') })).toBeNull()
      expect(screen.queryByLabelText(t('settings.password'))).toBeNull()
      await fireEvent.click(connect)
      expect(api.connect).toHaveBeenCalledOnce()
    })

    it('logged out: the banner on every page offers Connect', async () => {
      session({ kind: 'loggedOut' })
      credentials.status = SAVED
      render(LoginPanel, { variant: 'banner' })
      expect(screen.getByTestId('login-panel').textContent).toContain(t('login.title.loggedOut'))
      expect(screen.queryByRole('button', { name: t('login.change') })).toBeNull()
      await fireEvent.click(screen.getByRole('button', { name: t('settings.connect') }))
      expect(api.connect).toHaveBeenCalledOnce()
    })

    it('logged out with the password removed asks for it instead of offering Connect', () => {
      session({ kind: 'loggedOut' })
      credentials.status = NONE
      render(LoginPanel, { variant: 'settings' })
      expect(pwInput()).toBeTruthy()
      expect(screen.queryByRole('button', { name: t('settings.connect') })).toBeNull()
    })
  })
})
