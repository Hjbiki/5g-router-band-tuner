import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))
vi.mock('../lib/backend', () => ({
  api: { saveSettings: vi.fn(), saveCredentials: vi.fn(), connect: vi.fn(), credentialStatus: vi.fn() },
}))

import { api } from '../lib/backend'
import { app } from '../lib/store.svelte'
import { setLanguage, t } from '../lib/i18n'
import Onboarding from './Onboarding.svelte'
import { routerProbe } from '../lib/routerProbe.svelte'
import { settingsFixture, snap } from '../test-fixtures'
import { credentials } from '../lib/credentials.svelte'
import type { Settings as S } from '../lib/bindings/Settings'

const base: S = settingsFixture()

const next = () => fireEvent.click(screen.getByRole('button', { name: t('onboarding.next') }))
const skipLogin = () => fireEvent.click(screen.getByRole('button', { name: t('onboarding.loginSkip') }))
const loginTitle = () => screen.getByRole('heading', { name: t('onboarding.loginTitle') })

describe('Onboarding', () => {
  beforeEach(() => {
    app.settings = structuredClone(base)
    setLanguage('en')
    vi.mocked(api.saveSettings).mockResolvedValue(undefined)
    vi.mocked(api.saveCredentials).mockResolvedValue(undefined)
    vi.mocked(api.connect).mockResolvedValue(undefined)
    vi.mocked(api.credentialStatus).mockResolvedValue({ saved: true, rejected: false })
    app.snapshot = snap({ session: { kind: 'notConfigured' } })
    credentials.status = { saved: false, rejected: false }
  })
  afterEach(() => {
    cleanup()
    setLanguage('en')
    vi.clearAllMocks()
  })

  it('saves the password once, waits for the router to accept it, then goes on', async () => {
    render(Onboarding)
    await next()
    expect(screen.queryByLabelText(t('settings.username'))).toBeNull()
    await fireEvent.click(screen.getByRole('button', { name: t('login.changeUser') }))
    const user = screen.getByLabelText(t('settings.username')) as HTMLInputElement
    expect(user.value).toBe('admin')
    const pw = screen.getByLabelText(t('settings.password')) as HTMLInputElement
    await fireEvent.input(pw, { target: { value: 'secret' } })
    await fireEvent.click(screen.getByRole('button', { name: t('login.saveConnect') }))
    await waitFor(() => expect(api.connect).toHaveBeenCalledOnce())
    expect(api.saveCredentials).toHaveBeenCalledExactlyOnceWith('secret')
    expect(pw.value).toBe('')
    // The username is saved first: the keychain entry is named after it.
    expect(vi.mocked(api.saveSettings).mock.invocationCallOrder[0]).toBeLessThan(
      vi.mocked(api.saveCredentials).mock.invocationCallOrder[0],
    )
    // The settings sent never carry the password.
    expect(JSON.stringify(vi.mocked(api.saveSettings).mock.calls)).not.toContain('secret')

    // Still on step 2 while the router checks it.
    app.snapshot = snap({ session: { kind: 'connecting' } })
    await waitFor(() => expect(screen.getByText(t('login.checking'))).toBeTruthy())
    expect(loginTitle()).toBeTruthy()
    app.snapshot = snap({ session: { kind: 'loggedIn' } })
    await waitFor(() => expect(screen.getByText(t('onboarding.loginOk'))).toBeTruthy())
    await waitFor(() => screen.getByRole('heading', { name: t('onboarding.safetyTitle') }), { timeout: 2000 })

    await fireEvent.click(screen.getByRole('button', { name: t('onboarding.finish') }))
    await waitFor(() => expect(app.settings?.onboardingDone).toBe(true))
  })

  it('stays on step 2 with the error when the router rejects the password', async () => {
    render(Onboarding)
    await next()
    await fireEvent.input(screen.getByLabelText(t('settings.password')), { target: { value: 'wrong' } })
    await fireEvent.click(screen.getByRole('button', { name: t('login.saveConnect') }))
    await waitFor(() => expect(api.connect).toHaveBeenCalledOnce())
    app.snapshot = snap({ session: { kind: 'connecting' } })
    await waitFor(() => expect(screen.getByText(t('login.checking'))).toBeTruthy())
    app.snapshot = snap({ session: { kind: 'badCredentials' } })
    await waitFor(() => expect(screen.getByRole('alert').textContent).toContain(t('login.title.wrong')))
    expect(loginTitle()).toBeTruthy()
    // Back and Skip stay available; nothing is sent again.
    expect((screen.getByRole('button', { name: t('onboarding.back') }) as HTMLButtonElement).disabled).toBe(false)
    await skipLogin()
    await waitFor(() => screen.getByRole('heading', { name: t('onboarding.safetyTitle') }))
    expect(api.connect).toHaveBeenCalledOnce()
    expect(api.saveCredentials).toHaveBeenCalledOnce()
  })

  it('leaves the update check off unless it is ticked, and skip leaves it unanswered', async () => {
    render(Onboarding)
    await next()
    await skipLogin()
    await waitFor(() => screen.getByRole('heading', { name: t('onboarding.safetyTitle') }))
    await fireEvent.click(screen.getByRole('button', { name: t('onboarding.finish') }))
    await waitFor(() => expect(app.settings?.updateCheck).toBe(false))
    cleanup()

    app.settings = settingsFixture({ updateCheck: null, onboardingDone: false })
    render(Onboarding)
    await next()
    await skipLogin()
    await waitFor(() => screen.getByRole('heading', { name: t('onboarding.safetyTitle') }))
    await fireEvent.click(screen.getByRole('checkbox'))
    await fireEvent.click(screen.getByRole('button', { name: t('onboarding.finish') }))
    await waitFor(() => expect(app.settings?.updateCheck).toBe(true))
    cleanup()

    app.settings = settingsFixture({ updateCheck: null, onboardingDone: false })
    render(Onboarding)
    await fireEvent.click(screen.getByRole('button', { name: t('onboarding.skip') }))
    await waitFor(() => expect(app.settings?.onboardingDone).toBe(true))
    expect(app.settings?.updateCheck).toBeNull()
  })

  it('says a password is not needed on a router the app does not support', async () => {
    routerProbe.view = { support: 'unknown', vendor: null, model: null, hardware: null, firmware: null }
    render(Onboarding)
    await next()
    expect(screen.getByTestId('onb-unsupported').textContent).toBe(t('unsupported.onboarding'))
    routerProbe.view = null
  })

  it('goes on without a password and does not connect', async () => {
    render(Onboarding)
    await next()
    await skipLogin()
    await waitFor(() => screen.getByRole('heading', { name: t('onboarding.safetyTitle') }))
    expect(api.saveCredentials).not.toHaveBeenCalled()
    expect(api.connect).not.toHaveBeenCalled()
  })

  it("a new username skipped without a password re-reads that account's saved password", async () => {
    credentials.status = { saved: true, rejected: false }
    vi.mocked(api.credentialStatus).mockResolvedValue({ saved: false, rejected: false })
    render(Onboarding)
    await next()
    await fireEvent.click(screen.getByRole('button', { name: t('login.changeUser') }))
    await fireEvent.input(screen.getByLabelText(t('settings.username')), { target: { value: 'other' } })
    await skipLogin()
    await waitFor(() => screen.getByRole('heading', { name: t('onboarding.safetyTitle') }))
    expect(app.settings?.username).toBe('other')
    expect(api.credentialStatus).toHaveBeenCalledOnce()
    expect(credentials.status).toEqual({ saved: false, rejected: false })
    expect(api.connect).not.toHaveBeenCalled()
  })

  it('names the real Keep and Revert buttons and warns about the drop', async () => {
    render(Onboarding)
    await next()
    await skipLogin()
    await waitFor(() => screen.getByRole('heading', { name: t('onboarding.safetyTitle') }))
    expect(screen.getByText(new RegExp(t('safeApply.keep')))).toBeTruthy()
    expect(screen.getByText(new RegExp(t('safeApply.revert')))).toBeTruthy()
    expect(screen.getByRole('note').textContent).toBe(t('onboarding.safetyWarning'))
  })

  it('skip marks the guide done', async () => {
    render(Onboarding)
    await fireEvent.click(screen.getByRole('button', { name: t('onboarding.skip') }))
    await waitFor(() => expect(app.settings?.onboardingDone).toBe(true))
    expect(vi.mocked(api.saveSettings).mock.calls[0][0].onboardingDone).toBe(true)
  })

  it('switches to Arabic from the first screen and saves the choice', async () => {
    render(Onboarding)
    await fireEvent.click(screen.getByRole('button', { name: 'العربية' }))
    await waitFor(() => expect(app.settings?.language).toBe('ar'))
    expect(document.documentElement.dir).toBe('rtl')
    expect(screen.getByRole('heading', { name: t('onboarding.welcomeTitle') }).textContent).toContain('مرحبًا')
  })
})
