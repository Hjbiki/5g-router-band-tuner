import { describe, expect, it } from 'vitest'
import { fitZoom, stepUiScale, uiScaleFromWheel, uiScaleShortcut } from './uiScale'

const key = (init: KeyboardEventInit) => new KeyboardEvent('keydown', { ctrlKey: true, ...init })

describe('UI size', () => {
  it('steps through the sizes and stops at the ends', () => {
    expect(stepUiScale(100, 1)).toBe(110)
    expect(stepUiScale(100, -1)).toBe(90)
    expect(stepUiScale(150, 1)).toBe(150)
    expect(stepUiScale(90, -1)).toBe(90)
    // A size saved by hand between two steps moves to the next listed one.
    expect(stepUiScale(115, 1)).toBe(125)
    expect(stepUiScale(115, -1)).toBe(110)
  })

  it('reads Ctrl+=, Ctrl+- and Ctrl+0, also on an Arabic layout', () => {
    expect(uiScaleShortcut(key({ key: '=' }), 100)).toBe(110)
    expect(uiScaleShortcut(key({ key: '+' }), 100)).toBe(110)
    expect(uiScaleShortcut(key({ key: '-' }), 100)).toBe(90)
    expect(uiScaleShortcut(key({ key: '0' }), 125)).toBe(100)
    expect(uiScaleShortcut(key({ key: '٠', code: 'Digit0' }), 125)).toBe(100)
    expect(uiScaleShortcut(key({ key: 'x', code: 'Minus' }), 100)).toBe(90)
  })

  it('reads Ctrl + wheel, up for larger', () => {
    expect(uiScaleFromWheel({ ctrlKey: true, deltaY: -100 }, 100)).toBe(110)
    expect(uiScaleFromWheel({ ctrlKey: true, deltaY: 100 }, 100)).toBe(90)
    expect(uiScaleFromWheel({ ctrlKey: false, deltaY: 100 }, 100)).toBeNull()
  })

  it('leaves everything else alone', () => {
    expect(uiScaleShortcut(new KeyboardEvent('keydown', { key: '=' }), 100)).toBeNull()
    expect(uiScaleShortcut(key({ key: 'a' }), 100)).toBeNull()
    expect(uiScaleShortcut(key({ key: '=', altKey: true }), 100)).toBeNull()
  })
})

describe('a design that fills the window', () => {
  it('never goes below 100% and stops at 175%', () => {
    expect(fitZoom(1200, 700)).toBe(1)
    expect(fitZoom(1500, 880)).toBe(1)
    expect(fitZoom(8000, 4000)).toBe(1.75)
  })
  it('fills a 2560 by 1400 window at 160% and a 1920 by 1040 one at 120%', () => {
    expect(fitZoom(2560, 1400)).toBeCloseTo(1.6)
    expect(fitZoom(1920, 1040)).toBeCloseTo(1.2)
  })
  it('is held back by the shorter side', () => {
    expect(fitZoom(3000, 900)).toBe(1)
  })
})
