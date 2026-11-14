import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))
vi.mock('../lib/backend', () => ({
  api: { preparePageScripts: vi.fn(), savePageScripts: vi.fn() },
}))

import { api } from '../lib/backend'
import { t } from '../lib/i18n'
import { app } from '../lib/store.svelte'
import type { AppSnapshot } from '../lib/bindings/AppSnapshot'
import type { PageScriptsSummary } from '../lib/bindings/PageScriptsSummary'
import PageScriptsExport from './PageScriptsExport.svelte'

const SUMMARY: PageScriptsSummary = {
  files: 6,
  bytes: 4096,
  zipBytes: 2048,
  skipped: [{ path: '/js/hidden.js', reason: 'stillFlagged' }],
  missing: ['/js/missing.js'],
  stopped: null,
}

function session(kind: 'loggedIn' | 'loggedOut') {
  app.snapshot = { session: { kind } } as unknown as AppSnapshot
}

describe('PageScriptsExport', () => {
  beforeEach(() => session('loggedIn'))
  afterEach(() => {
    cleanup()
    vi.clearAllMocks()
    app.snapshot = null
  })

  it('says what it does, that it only reads, and that the file stays on this PC', () => {
    render(PageScriptsExport)
    expect(screen.getByText(t('pageScripts.body'))).toBeTruthy()
    expect(t('pageScripts.body')).toContain('only reads')
    expect(t('pageScripts.body')).toContain('on this PC')
    expect(t('pageScripts.body')).toContain('sends nothing')
  })

  it('shows what the zip holds before it can be saved', async () => {
    vi.mocked(api.preparePageScripts).mockResolvedValue(SUMMARY)
    vi.mocked(api.savePageScripts).mockResolvedValue(true)
    render(PageScriptsExport)
    expect(screen.queryByRole('button', { name: t('pageScripts.save') })).toBeNull()

    await fireEvent.click(screen.getByRole('button', { name: t('pageScripts.read') }))
    await waitFor(() =>
      expect(screen.getByText(t('pageScripts.files', { count: '6', size: '4' }))).toBeTruthy(),
    )
    expect(screen.getByText(t('pageScripts.missing', { count: '1' }))).toBeTruthy()
    expect(screen.getByText('/js/hidden.js')).toBeTruthy()
    // The reason is worded in the app's language, not the backend's English.
    expect(screen.getByText(t('pageScripts.reason.stillFlagged'))).toBeTruthy()
    expect(api.savePageScripts).not.toHaveBeenCalled()

    await fireEvent.click(screen.getByRole('button', { name: t('pageScripts.save') }))
    await waitFor(() => expect(api.savePageScripts).toHaveBeenCalledOnce())
    await waitFor(() => expect(screen.queryByRole('button', { name: t('pageScripts.save') })).toBeNull())
  })

  it('cancel drops the summary without saving', async () => {
    vi.mocked(api.preparePageScripts).mockResolvedValue(SUMMARY)
    render(PageScriptsExport)
    await fireEvent.click(screen.getByRole('button', { name: t('pageScripts.read') }))
    await waitFor(() => screen.getByRole('button', { name: t('app.cancel') }))
    await fireEvent.click(screen.getByRole('button', { name: t('app.cancel') }))
    expect(screen.getByRole('button', { name: t('pageScripts.read') })).toBeTruthy()
    expect(api.savePageScripts).not.toHaveBeenCalled()
  })

  it('is off while logged out', () => {
    session('loggedOut')
    render(PageScriptsExport)
    const button = screen.getByRole('button', { name: t('pageScripts.read') }) as HTMLButtonElement
    expect(button.disabled).toBe(true)
    expect(screen.getByText(t('pageScripts.needLogin'))).toBeTruthy()
  })

  it('shows a refusal in words', async () => {
    vi.mocked(api.preparePageScripts).mockRejectedValue(
      'Wait until the current band change, Refresh IP, APN change, auto-scan or speed test has finished.',
    )
    render(PageScriptsExport)
    await fireEvent.click(screen.getByRole('button', { name: t('pageScripts.read') }))
    await waitFor(() => expect(screen.getByRole('alert').textContent).toBe(t('pageScripts.refuse.busy')))
    expect(screen.queryByRole('button', { name: t('pageScripts.save') })).toBeNull()
  })
})
