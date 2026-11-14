import { afterEach, describe, expect, it, vi } from 'vitest'
import { fireEvent, render, screen, cleanup, within } from '@testing-library/svelte'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))
vi.mock('@tauri-apps/api/app', () => ({ getVersion: vi.fn(() => Promise.resolve('9.9.9')) }))
vi.mock('../lib/backend', () => ({
  api: {
    openDeveloperPage: vi.fn(() => Promise.resolve()),
    requestReboot: vi.fn(),
    confirmReboot: vi.fn(),
    updateStatus: vi.fn(() => Promise.resolve({ current: '9.9.9', available: null })),
    requestFactoryReset: vi.fn(),
    confirmFactoryReset: vi.fn(),
    privacyCheck: vi.fn(),
    dnsCheck: vi.fn(),
  },
  onUpdate: vi.fn(() => Promise.resolve(() => {})),
}))

import { api } from '../lib/backend'
import { t } from '../lib/i18n'
import RouterPage from './RouterPage.svelte'

describe('RouterPage', () => {
  afterEach(() => {
    cleanup()
    vi.clearAllMocks()
  })

  it('has the restart with the factory reset beside it, and the page scripts', () => {
    render(RouterPage)
    expect(screen.getByRole('heading', { name: t('router.title') })).toBeTruthy()
    expect(screen.getByRole('heading', { name: t('pageScripts.title') })).toBeTruthy()
    // Both buttons sit under the one maintenance heading; there is no separate folded section.
    const maintenance = screen.getByTestId('maintenance')
    expect(within(maintenance).getByRole('button', { name: t('reboot.request') })).toBeTruthy()
    expect(within(maintenance).getByRole('button', { name: t('danger.open') })).toBeTruthy()
    expect(document.querySelector('details.danger-wrap')).toBeNull()
  })

  it('has the privacy check, which reads nothing by itself', () => {
    render(RouterPage)
    expect(screen.getByRole('heading', { name: t('privacy.title') })).toBeTruthy()
    expect(screen.getByRole('button', { name: t('privacy.run') })).toBeTruthy()
    expect(api.privacyCheck).not.toHaveBeenCalled()
    expect(api.dnsCheck).not.toHaveBeenCalled()
  })

  it('keeps factory reset off on an unverified router', () => {
    render(RouterPage)
    expect((screen.getByRole('button', { name: t('danger.open') }) as HTMLButtonElement).disabled).toBe(true)
    expect(screen.getByTestId('reset-blocked').textContent).toContain(t('danger.notVerified'))
  })
})
