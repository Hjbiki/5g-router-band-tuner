import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))
vi.mock('../lib/backend', () => ({
  api: {
    updateStatus: vi.fn(),
    checkForUpdates: vi.fn(),
    dismissUpdate: vi.fn(),
    openReleasePage: vi.fn(),
    saveSettings: vi.fn(),
  },
  onUpdate: vi.fn(),
}))

import { api, onUpdate } from '../lib/backend'
import { app } from '../lib/store.svelte'
import { t } from '../lib/i18n'
import { resetUpdates } from '../lib/updates.svelte'
import { settingsFixture } from '../test-fixtures'
import UpdateCard from './UpdateCard.svelte'

const none = { current: '0.1.0', available: null }
const newer = { current: '0.1.0', available: { version: '1.2.3', summary: 'Better <b>bands</b>.' } }

describe('UpdateCard', () => {
  beforeEach(() => {
    resetUpdates()
    app.settings = settingsFixture({ updateCheck: false })
    vi.mocked(onUpdate).mockResolvedValue(() => {})
    vi.mocked(api.updateStatus).mockResolvedValue(none)
    vi.mocked(api.saveSettings).mockResolvedValue(undefined)
    vi.mocked(api.openReleasePage).mockResolvedValue(undefined)
  })
  afterEach(() => {
    cleanup()
    vi.clearAllMocks()
  })

  it('makes no check by itself', async () => {
    render(UpdateCard)
    await waitFor(() => expect(api.updateStatus).toHaveBeenCalledOnce())
    expect(api.checkForUpdates).not.toHaveBeenCalled()
    expect(screen.queryByTestId('update-notice')).toBeNull()
  })

  it('says when there is nothing newer, and when GitHub could not be reached', async () => {
    vi.mocked(api.checkForUpdates).mockResolvedValueOnce(none)
    render(UpdateCard)
    await fireEvent.click(screen.getByRole('button', { name: t('updates.check') }))
    await waitFor(() => expect(screen.getByText(t('updates.upToDate'))).toBeTruthy())

    vi.mocked(api.checkForUpdates).mockRejectedValueOnce("couldn't reach GitHub")
    await fireEvent.click(screen.getByRole('button', { name: t('updates.check') }))
    await waitFor(() => expect(screen.getByText(t('updates.unreachable'))).toBeTruthy())
  })

  it('shows the notice with its notes as plain text, and opens the page through the backend', async () => {
    vi.mocked(api.updateStatus).mockResolvedValue(newer)
    render(UpdateCard)
    const notice = await screen.findByTestId('update-notice')
    expect(notice.textContent).toContain(t('updates.available', { version: '1.2.3' }))
    expect(notice.textContent).toContain('Better <b>bands</b>.')
    expect(notice.querySelector('b')).toBeNull()
    await fireEvent.click(screen.getByRole('button', { name: t('updates.open') }))
    expect(api.openReleasePage).toHaveBeenCalledOnce()
  })

  it('can be dismissed', async () => {
    vi.mocked(api.updateStatus).mockResolvedValue(newer)
    vi.mocked(api.dismissUpdate).mockResolvedValue(none)
    render(UpdateCard)
    await screen.findByTestId('update-notice')
    await fireEvent.click(screen.getByRole('button', { name: t('app.dismiss') }))
    expect(api.dismissUpdate).toHaveBeenCalledWith('1.2.3')
    await waitFor(() => expect(screen.queryByTestId('update-notice')).toBeNull())
  })

  it('asks once when nobody has answered yet, and remembers either answer', async () => {
    app.settings = settingsFixture({ updateCheck: null })
    render(UpdateCard)
    expect(screen.getByTestId('update-question')).toBeTruthy()
    await fireEvent.click(screen.getByRole('button', { name: t('updates.allow') }))
    await waitFor(() => expect(app.settings?.updateCheck).toBe(true))
    expect(screen.queryByTestId('update-question')).toBeNull()
    cleanup()

    app.settings = settingsFixture({ updateCheck: null })
    render(UpdateCard)
    await fireEvent.click(screen.getByRole('button', { name: t('updates.notNow') }))
    await waitFor(() => expect(app.settings?.updateCheck).toBe(false))
  })
})
