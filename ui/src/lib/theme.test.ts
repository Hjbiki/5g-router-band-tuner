import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { applyTheme, followSystemTheme, resolveTheme } from './theme'

// A controllable stand-in for matchMedia('(prefers-color-scheme: dark)').
function fakeMedia(dark: boolean) {
  const listeners = new Set<() => void>()
  const mql = {
    matches: dark,
    addEventListener: (_: string, cb: () => void) => listeners.add(cb),
    removeEventListener: (_: string, cb: () => void) => listeners.delete(cb),
  }
  vi.stubGlobal(
    'matchMedia',
    vi.fn(() => mql),
  )
  return {
    set(next: boolean) {
      mql.matches = next
      for (const cb of listeners) cb()
    },
    listeners,
  }
}

describe('theme', () => {
  const root = document.documentElement
  beforeEach(() => {
    delete root.dataset.theme
    delete root.dataset.palette
  })
  afterEach(() => vi.unstubAllGlobals())

  it('resolves system against the OS setting, and keeps explicit choices', () => {
    expect(resolveTheme('system', true)).toBe('dark')
    expect(resolveTheme('system', false)).toBe('light')
    expect(resolveTheme('light', true)).toBe('light')
    expect(resolveTheme('dark', false)).toBe('dark')
  })

  it('sets data-theme and data-palette on <html>', () => {
    fakeMedia(false)
    applyTheme('dark', 'cobalt')
    expect(root.dataset.theme).toBe('dark')
    expect(root.dataset.palette).toBe('cobalt')
    applyTheme('system', 'sandstone')
    expect(root.dataset.theme).toBe('light')
    expect(root.dataset.palette).toBe('sandstone')
  })

  it('falls back to Carbon and System for missing or unknown values', () => {
    fakeMedia(true)
    applyTheme(undefined, 'neon' as never)
    expect(root.dataset.palette).toBe('carbon')
    expect(root.dataset.theme).toBe('dark')
  })

  it('follows the OS live while the choice is system, and stops after dispose', () => {
    const media = fakeMedia(false)
    applyTheme('system', 'carbon')
    const off = followSystemTheme()
    media.set(true)
    expect(root.dataset.theme).toBe('dark')
    media.set(false)
    expect(root.dataset.theme).toBe('light')

    applyTheme('dark', 'carbon')
    media.set(false)
    expect(root.dataset.theme).toBe('dark')

    off()
    expect(media.listeners.size).toBe(0)
  })

  it('works without matchMedia', () => {
    vi.stubGlobal('matchMedia', undefined)
    applyTheme('system', 'carbon')
    expect(root.dataset.theme).toBe('light')
    expect(followSystemTheme()).toBeTypeOf('function')
  })
})
