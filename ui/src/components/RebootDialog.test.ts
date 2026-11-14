import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { fireEvent, render, screen, cleanup } from '@testing-library/svelte'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))
vi.mock('../lib/backend', () => ({
  api: { requestReboot: vi.fn(), confirmReboot: vi.fn() },
}))

import { api } from '../lib/backend'
import { app } from '../lib/store.svelte'
import { t } from '../lib/i18n'
import RebootDialog from './RebootDialog.svelte'
import { snap } from '../test-fixtures'
import type { Phase } from '../lib/bindings/Phase'

const setPhase = (phase: Phase) => {
  app.snapshot = snap({ safeApply: { phase, secondsLeft: null, previous: null, requested: null } })
}

describe('RebootDialog', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    vi.mocked(api.requestReboot).mockResolvedValue({ id: 'ticket-1', expiresInS: 30 })
    vi.mocked(api.confirmReboot).mockResolvedValue(undefined)
    setPhase({ kind: 'idle' })
  })
  afterEach(() => {
    cleanup()
    vi.useRealTimers()
    vi.clearAllMocks()
  })

  const open = async () => {
    render(RebootDialog)
    await fireEvent.click(screen.getByRole('button', { name: t('reboot.request') }))
    await vi.advanceTimersByTimeAsync(0)
  }

  it('does not call confirmReboot without a ticket', async () => {
    render(RebootDialog)
    expect(screen.queryByRole('dialog')).toBeNull()
    expect(api.confirmReboot).not.toHaveBeenCalled()
  })

  it('confirms with the ticket id', async () => {
    await open()
    await fireEvent.click(screen.getByRole('button', { name: t('reboot.confirm') }))
    expect(api.confirmReboot).toHaveBeenCalledWith('ticket-1')
  })

  it('closes after 30 s without confirming', async () => {
    await open()
    expect(screen.getByRole('dialog')).toBeTruthy()
    await vi.advanceTimersByTimeAsync(30_000)
    expect(screen.queryByRole('dialog')).toBeNull()
    expect(api.confirmReboot).not.toHaveBeenCalled()
  })

  it('disables Confirm while a probation is running', async () => {
    await open()
    setPhase({ kind: 'probation' })
    await vi.advanceTimersByTimeAsync(0)
    const btn = screen.getByRole('button', { name: t('reboot.confirm') }) as HTMLButtonElement
    expect(btn.disabled).toBe(true)
    await fireEvent.click(btn)
    expect(api.confirmReboot).not.toHaveBeenCalled()
  })

  it('does not even request a ticket during a safe-apply', async () => {
    setPhase({ kind: 'probation' })
    render(RebootDialog)
    expect((screen.getByRole('button', { name: t('reboot.request') }) as HTMLButtonElement).disabled).toBe(true)
  })
})
