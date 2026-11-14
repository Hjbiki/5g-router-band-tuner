import { getCurrentWebview } from '@tauri-apps/api/webview'
import { getCurrentWindow } from '@tauri-apps/api/window'

/** The sizes Settings offers, in percent; Ctrl+= and Ctrl+- step through them. */
export const UI_SCALES: readonly number[] = [90, 100, 110, 125, 150]
export const UI_SCALE_DEFAULT = 100

// The window size a fitted design is drawn for. A larger window zooms in until it is filled, so a
// big screen isn't mostly empty; the user's UI size multiplies on top of that.
const FIT_WIDTH = 1500
const FIT_HEIGHT = 880
const FIT_MAX = 1.75
const FIT_STEP = 0.05

/** The zoom that fills a window of this size (in pixels at 100%), never below 100%. */
export function fitZoom(width: number, height: number): number {
  const z = Math.min(width / FIT_WIDTH, height / FIT_HEIGHT, FIT_MAX)
  return Math.max(1, Math.round(z / FIT_STEP) * FIT_STEP)
}

// The last choice applied and the zoom last sent, so an unchanged one makes no call into the window.
let current: { percent: number; fit: boolean } | null = null
let zoom: number | null = null
function setZoom(z: number): void {
  z = Math.round(z * 1000) / 1000
  if (z === zoom) return
  zoom = z
  try {
    getCurrentWebview()
      .setZoom(z)
      .catch(() => {})
  } catch {
    // Outside the app window (tests, a plain browser) there is nothing to zoom.
  }
}

/** The window's size comes from physical pixels, so it doesn't depend on the zoom being set. */
async function refit(): Promise<void> {
  const mine = current
  if (!mine?.fit) return
  let scale = 1
  try {
    scale = await getCurrentWindow().scaleFactor()
  } catch {
    // No window API: take the display scale as 1.
  }
  if (current !== mine || !(scale > 0)) return
  const dpr = window.devicePixelRatio || 1
  setZoom((mine.percent / 100) * fitZoom((window.innerWidth * dpr) / scale, (window.innerHeight * dpr) / scale))
}

let resizeTimer: ReturnType<typeof setTimeout> | undefined
let watching = false
function onResize(): void {
  if (resizeTimer) clearTimeout(resizeTimer)
  resizeTimer = setTimeout(() => void refit(), 150)
}

/**
 * Zooms the whole webview, so the layout rules see the smaller or larger page too. With `fit`
 * (a design that fills the window), the zoom also grows with the window.
 */
export function applyUiScale(percent: number | undefined, fit = false): void {
  if (percent === undefined || (current?.percent === percent && current.fit === fit)) return
  current = { percent, fit }
  if (!fit) return setZoom(percent / 100)
  if (!watching && typeof window !== 'undefined') {
    window.addEventListener('resize', onResize)
    watching = true
  }
  void refit()
}

/** The next size up or down from `percent`, staying at the ends of the list. */
export function stepUiScale(percent: number, direction: 1 | -1): number {
  if (direction > 0) return UI_SCALES.find((s) => s > percent) ?? UI_SCALES[UI_SCALES.length - 1]
  return [...UI_SCALES].reverse().find((s) => s < percent) ?? UI_SCALES[0]
}

/** What Ctrl + wheel asks for: the new size, or null without Ctrl. Up is larger. */
export function uiScaleFromWheel(e: Pick<WheelEvent, 'ctrlKey' | 'deltaY'>, percent: number): number | null {
  if (!e.ctrlKey || e.deltaY === 0) return null
  return stepUiScale(percent, e.deltaY < 0 ? 1 : -1)
}

/** What a key press asks for: the new size, or null when it isn't a size shortcut. */
export function uiScaleShortcut(e: KeyboardEvent, percent: number): number | null {
  if (!e.ctrlKey || e.altKey || e.metaKey) return null
  // `code` as well, so the shortcuts survive an Arabic keyboard layout.
  if (e.key === '=' || e.key === '+' || e.code === 'Equal' || e.code === 'NumpadAdd') return stepUiScale(percent, 1)
  if (e.key === '-' || e.code === 'Minus' || e.code === 'NumpadSubtract') return stepUiScale(percent, -1)
  if (e.key === '0' || e.code === 'Digit0' || e.code === 'Numpad0') return UI_SCALE_DEFAULT
  return null
}
