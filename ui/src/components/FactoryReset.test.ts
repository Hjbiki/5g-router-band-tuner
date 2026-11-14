import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))
vi.mock('../lib/backend', () => ({
  api: { requestFactoryReset: vi.fn(), confirmFactoryReset: vi.fn() },
}))

import { api } from '../lib/backend'
import { app } from '../lib/store.svelte'
import { speed } from '../lib/speedtest.svelte'
import { t } from '../lib/i18n'
import { snap, settingsFixture } from '../test-fixtures'
import FactoryReset from './FactoryReset.svelte'
import type { AppSnapshot } from '../lib/bindings/AppSnapshot'

function verified(o: Partial<AppSnapshot> = {}): AppSnapshot {
  const s = snap(o)
  return { ...s, capabilities: { ...s.capabilities!, factoryResetVerified: true } }
}
const openBtn = () => screen.getByRole('button', { name: t('danger.open') }) as HTMLButtonElement
const warning = () => screen.queryByRole('alertdialog')
const bandChange = { safeApply: { phase: { kind: 'probation' }, secondsLeft: 10, previous: null, requested: null } as never }

describe('FactoryReset', () => {
  beforeEach(() => {
    app.settings = settingsFixture()
    vi.mocked(api.requestFactoryReset).mockResolvedValue({ id: 'ticket', expiresInS: 30 })
    vi.mocked(api.confirmFactoryReset).mockResolvedValue(undefined)
    speed.running = false
  })
  afterEach(() => {
    cleanup()
    vi.clearAllMocks()
    vi.useRealTimers()
    app.snapshot = null
  })

  it('warns first, in a few words, and resets nothing by opening the warning', async () => {
    app.snapshot = verified()
    render(FactoryReset)
    expect(warning()).toBeNull()
    await fireEvent.click(openBtn())
    const dialog = await screen.findByRole('alertdialog')
    expect(within(dialog).getByRole('heading', { name: t('danger.title') })).toBeTruthy()
    expect(within(dialog).getByText(t('danger.body'))).toBeTruthy()
    expect(api.confirmFactoryReset).not.toHaveBeenCalled()
  })

  it('resets on the confirm button alone, with no word to type', async () => {
    vi.useFakeTimers()
    app.snapshot = verified()
    render(FactoryReset)
    await fireEvent.click(openBtn())
    await vi.advanceTimersByTimeAsync(1100)
    const dialog = warning()!
    expect(within(dialog).queryByRole('textbox')).toBeNull()
    await fireEvent.click(within(dialog).getByRole('button', { name: t('danger.confirm') }))
    await vi.advanceTimersByTimeAsync(0)
    expect(api.confirmFactoryReset).toHaveBeenCalledWith('ticket')
    expect(warning()).toBeNull()
  })

  it('arms the confirm button a moment after the warning opens, so a double-click resets nothing', async () => {
    vi.useFakeTimers()
    app.snapshot = verified()
    render(FactoryReset)
    await fireEvent.click(openBtn())
    await vi.advanceTimersByTimeAsync(50)
    const confirm = within(warning()!).getByRole('button', { name: t('danger.confirm') }) as HTMLButtonElement
    // The second click of a double-click lands here, on a button that isn't live yet.
    expect(confirm.disabled).toBe(true)
    await fireEvent.click(confirm)
    await vi.advanceTimersByTimeAsync(500)
    expect(api.confirmFactoryReset).not.toHaveBeenCalled()
    expect(confirm.disabled).toBe(true)
    await vi.advanceTimersByTimeAsync(600)
    expect(confirm.disabled).toBe(false)
    // Cancel works from the first moment.
    expect((within(warning()!).getByRole('button', { name: t('app.cancel') }) as HTMLButtonElement).disabled).toBe(false)
  })

  it('cancel closes the warning and resets nothing', async () => {
    app.snapshot = verified()
    render(FactoryReset)
    await fireEvent.click(openBtn())
    const dialog = await screen.findByRole('alertdialog')
    await fireEvent.click(within(dialog).getByRole('button', { name: t('app.cancel') }))
    expect(warning()).toBeNull()
    expect(api.confirmFactoryReset).not.toHaveBeenCalled()
  })

  it('stays off on a router model whose reset call is not verified', async () => {
    app.snapshot = snap()
    render(FactoryReset)
    expect(screen.getByTestId('reset-blocked').textContent).toContain(t('danger.notVerified'))
    expect(openBtn().disabled).toBe(true)
    await fireEvent.click(openBtn())
    expect(api.requestFactoryReset).not.toHaveBeenCalled()
    expect(warning()).toBeNull()
  })

  it('takes the warning back when the 30 s run out', async () => {
    vi.useFakeTimers()
    app.snapshot = verified()
    render(FactoryReset)
    await fireEvent.click(openBtn())
    await vi.advanceTimersByTimeAsync(100)
    expect(warning()).not.toBeNull()
    await vi.advanceTimersByTimeAsync(31_000)
    expect(warning()).toBeNull()
    expect(api.confirmFactoryReset).not.toHaveBeenCalled()
  })

  it('is off during a band change and during a speed test', async () => {
    app.snapshot = verified(bandChange)
    render(FactoryReset)
    expect(screen.getByTestId('reset-blocked').textContent).toBe(t('danger.blockedBand'))
    expect(openBtn().disabled).toBe(true)
    cleanup()

    app.snapshot = verified()
    speed.running = true
    render(FactoryReset)
    expect(screen.getByTestId('reset-blocked').textContent).toBe(t('danger.blockedSpeed'))
    expect(openBtn().disabled).toBe(true)
  })

  it('a band change that starts while the warning is open closes it', async () => {
    app.snapshot = verified()
    render(FactoryReset)
    await fireEvent.click(openBtn())
    await screen.findByRole('alertdialog')
    app.snapshot = verified(bandChange)
    await waitFor(() => expect(warning()).toBeNull())
    expect(api.confirmFactoryReset).not.toHaveBeenCalled()
  })
})
