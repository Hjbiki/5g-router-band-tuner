import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/svelte'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))

import { app } from '../lib/store.svelte'
import { t } from '../lib/i18n'
import { scan } from '../lib/scan.svelte'
import { resetSpeedTest, speed } from '../lib/speedtest.svelte'
import Sidebar from './Sidebar.svelte'
import { credentials } from '../lib/credentials.svelte'
import { settingsFixture, snap } from '../test-fixtures'

describe('Sidebar', () => {
  beforeEach(() => {
    app.tab = 'dashboard'
    app.settings = settingsFixture()
    app.snapshot = snap({
      status: {
        connection: { kind: 'connected' },
        networkType: 19,
        operator: { name: 'Test Operator', mccMnc: '00101' },
        bars: 4,
        nrBars: null,
      },
    })
  })
  afterEach(() => cleanup())

  it('marks Auto-scan while a scan runs, wherever the user is', () => {
    render(Sidebar)
    expect(screen.getByRole('button', { name: t('app.scan') }).querySelector('.update-dot')).toBeNull()
    cleanup()
    scan.file = { status: 'running' } as never
    render(Sidebar)
    expect(screen.getByRole('button', { name: t('app.scan') }).querySelector('.update-dot')).not.toBeNull()
    scan.file = null
  })

  it('marks Speed test while a test runs, but not during a scan', () => {
    resetSpeedTest()
    render(Sidebar)
    const item = () => screen.getByRole('button', { name: t('app.speedtest') })
    expect(item().querySelector('.update-dot')).toBeNull()
    cleanup()
    speed.running = true
    render(Sidebar)
    expect(item().querySelector('.update-dot')).not.toBeNull()
    cleanup()
    scan.file = { status: 'running' } as never
    render(Sidebar)
    expect(item().querySelector('.update-dot')).toBeNull()
    scan.file = null
    resetSpeedTest()
  })

  it('lists every section and marks the current one', async () => {
    render(Sidebar)
    const nav = screen.getByRole('navigation', { name: t('app.navigation') })
    const names = [...nav.querySelectorAll('button')].map((b) => b.textContent?.trim())
    expect(names).toEqual([t('app.signal'), t('app.bands'), t('app.scan'), t('app.speedtest'), t('app.router'), t('app.history'), t('app.settings'), t('app.about')])
    expect(screen.getByRole('button', { name: t('app.signal') }).getAttribute('aria-current')).toBe('page')

    await fireEvent.click(screen.getByRole('button', { name: t('app.speedtest') }))
    expect(app.tab).toBe('speedtest')
    expect(screen.getByRole('button', { name: t('app.speedtest') }).getAttribute('aria-current')).toBe('page')
    expect(screen.getByRole('button', { name: t('app.signal') }).getAttribute('aria-current')).toBeNull()
  })

  it('shows the router, address, operator and connection status', () => {
    render(Sidebar)
    const aside = screen.getByRole('complementary', { name: t('shell.router') })
    expect(aside.textContent).toContain('H122-373')
    expect(aside.textContent).toContain('192.168.8.1')
    expect(aside.textContent).toContain('Test Operator')
    expect(screen.getByRole('status', { name: t('shell.status') }).textContent).toContain(t('shell.inService'))
  })

  it('shows the operator as a badge, with a neutral icon until there is a logo', () => {
    render(Sidebar)
    const badge = screen.getByTestId('operator-badge')
    expect(badge.textContent).toContain('Test Operator')
    expect(badge.querySelector('img')).toBeNull()
    expect(badge.querySelector('svg')).not.toBeNull()
  })

  it('says why when not logged in, in a few words', () => {
    app.snapshot = snap({ session: { kind: 'notConfigured' } })
    credentials.status = { saved: false, rejected: false }
    render(Sidebar)
    expect(screen.getByRole('status', { name: t('shell.status') }).textContent).toContain(t('login.short.needPassword'))
    cleanup()
    credentials.status = { saved: true, rejected: true }
    render(Sidebar)
    const status = screen.getByRole('status', { name: t('shell.status') })
    expect(status.textContent).toContain(t('login.short.wrong'))
    expect(status.querySelector('.pill.bad')).not.toBeNull()
    credentials.status = null
  })

  it('says logged out, with no uptime', () => {
    app.snapshot = snap({ session: { kind: 'loggedOut' }, status: null })
    credentials.status = { saved: true, rejected: false }
    render(Sidebar)
    const status = screen.getByRole('status', { name: t('shell.status') })
    expect(status.textContent).toContain(t('login.short.loggedOut'))
    expect(status.querySelector('.pill.warn')).not.toBeNull()
    expect(document.querySelector('.details')).toBeNull()
    credentials.status = null
  })
})
