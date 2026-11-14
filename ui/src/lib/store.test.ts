import { describe, expect, it, vi } from 'vitest'
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))
vi.mock('./backend', () => ({ api: { saveSettings: vi.fn() }, onAlert: vi.fn(), onSnapshot: vi.fn() }))
import { api } from './backend'
import { app, mergeEdits, pushToast, updateSettings } from './store.svelte'
import { settingsFixture } from '../test-fixtures'

describe('toasts', () => {
  it('keeps only the 5 newest', () => {
    for (let i = 1; i <= 7; i++) pushToast(`t${i}`)
    expect(app.toasts.map((x) => x.text)).toEqual(['t3', 't4', 't5', 't6', 't7'])
  })

  it('evicts info toasts before errors', () => {
    app.toasts.splice(0)
    pushToast('login failed', 'error')
    for (let i = 1; i <= 5; i++) pushToast(`i${i}`)
    expect(app.toasts.map((x) => x.text)).toEqual(['login failed', 'i2', 'i3', 'i4', 'i5'])
  })
})

describe('updateSettings', () => {
  it('runs saves one at a time, each on the newest settings', async () => {
    app.settings = settingsFixture()
    let release!: () => void
    vi.mocked(api.saveSettings)
      .mockImplementationOnce(() => new Promise<void>((r) => (release = r)))
      .mockResolvedValue(undefined)
    const a = updateSettings((s) => ({ ...s, automation: { ...s.automation, primaryBand: 3 } }))
    const b = updateSettings((s) => ({ ...s, cellAlerts: false }))
    await new Promise((r) => setTimeout(r, 0))
    expect(api.saveSettings).toHaveBeenCalledOnce()
    release()
    await Promise.all([a, b])
    expect(vi.mocked(api.saveSettings).mock.calls[1][0].automation.primaryBand).toBe(3)
    expect(app.settings?.automation.primaryBand).toBe(3)
    expect(app.settings?.cellAlerts).toBe(false)
  })

  it('a refused save rejects, keeps the store, and does not block the next one', async () => {
    app.settings = settingsFixture()
    vi.mocked(api.saveSettings).mockReset()
    vi.mocked(api.saveSettings).mockRejectedValueOnce('disk full').mockResolvedValue(undefined)
    await expect(updateSettings((s) => ({ ...s, host: 'x' }))).rejects.toBe('disk full')
    expect(app.settings?.host).toBe('192.168.8.1')
    await updateSettings((s) => ({ ...s, username: 'u' }))
    expect(app.settings?.username).toBe('u')
  })
})

describe('mergeEdits', () => {
  it('applies only the edited fields onto the current settings', () => {
    const base = settingsFixture()
    const edited = { ...settingsFixture(), cellAlerts: false, timers: { ...base.timers, graceS: 120 } }
    const current = {
      ...settingsFixture(),
      host: '10.0.0.9',
      driver: 'mock' as const,
      timers: { ...base.timers, lossS: 90 },
      automation: { ...base.automation, primaryBand: 7 },
    }
    const out = mergeEdits(base, edited, current)
    expect(out.host).toBe('10.0.0.9')
    expect(out.driver).toBe('mock')
    expect(out.cellAlerts).toBe(false)
    expect(out.timers).toEqual({ graceS: 120, lossS: 90, probationS: 180 })
    expect(out.automation.primaryBand).toBe(7)
  })
})
