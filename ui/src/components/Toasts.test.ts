import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/svelte'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))

import { app, pushToast } from '../lib/store.svelte'
import { t } from '../lib/i18n'
import Toasts from './Toasts.svelte'

describe('Toasts', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    app.toasts.splice(0)
  })
  afterEach(() => {
    cleanup()
    vi.useRealTimers()
  })

  it('info toasts time out; error toasts stay until dismissed', async () => {
    pushToast('saved')
    pushToast('login failed', 'error')
    render(Toasts)
    await vi.advanceTimersByTimeAsync(60_000)
    expect(screen.queryByText('saved')).toBeNull()
    expect(screen.getByText('login failed')).toBeTruthy()

    await fireEvent.click(screen.getByRole('button', { name: t('app.dismiss') }))
    expect(screen.queryByText('login failed')).toBeNull()
  })
})
