import { afterEach, describe, expect, it, vi } from 'vitest'
import { fireEvent, render, screen, cleanup } from '@testing-library/svelte'

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
    captureStatus: vi.fn(() => Promise.resolve({ phase: 'idle', requests: 0, limit: null })),
  },
  onUpdate: vi.fn(() => Promise.resolve(() => {})),
  onCaptureStatus: vi.fn(() => Promise.resolve(() => {})),
}))

import { api } from '../lib/backend'
import { setLanguage, t } from '../lib/i18n'
import About from './About.svelte'

describe('About', () => {
  afterEach(() => {
    cleanup()
    vi.clearAllMocks()
  })

  it('opens the developer page through the backend, not the webview', async () => {
    render(About)
    const link = screen.getByRole('link', { name: 'Anad Askar' })
    expect(link.getAttribute('href')).toBe('https://creators.sa/hibiki')
    const click = new MouseEvent('click', { bubbles: true, cancelable: true })
    await fireEvent(link, click)
    expect(click.defaultPrevented).toBe(true)
    expect(api.openDeveloperPage).toHaveBeenCalledOnce()
  })

  it("writes the developer's name in the interface language", () => {
    setLanguage('ar')
    render(About)
    expect(screen.getByRole('link', { name: 'عناد عسكر' })).toBeTruthy()
    setLanguage('en')
  })

  it('offers a recording of the router page beside Export diagnostics', () => {
    render(About)
    expect(screen.getByRole('heading', { name: new RegExp(t('recording.title')) })).toBeTruthy()
    expect(screen.getByRole('button', { name: t('recording.start') })).toBeTruthy()
  })

  it('leaves the router tools to the Router page', () => {
    render(About)
    expect(screen.queryByRole('heading', { name: t('danger.title') })).toBeNull()
    expect(screen.queryByRole('heading', { name: t('about.maintenance') })).toBeNull()
  })
})
