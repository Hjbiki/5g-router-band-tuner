import type { Palette } from './bindings/Palette'
import type { ThemeMode } from './bindings/ThemeMode'
import type { Design } from './bindings/Design'

/** Every palette in app.css, the default first. */
export const PALETTES: readonly Palette[] = ['carbon', 'cobalt', 'sandstone']
export const THEMES: readonly ThemeMode[] = ['system', 'light', 'dark']
/** Every design of the Signal and Bands pages, the default first. */
export const DESIGNS: readonly Design[] = ['classic', 'spectrum']

const DARK_QUERY = '(prefers-color-scheme: dark)'

// The last choice applied, so a system colour-scheme change can re-resolve "system".
let current: { mode: ThemeMode; palette: Palette; design: Design } = { mode: 'system', palette: 'carbon', design: 'classic' }

function systemQuery(): MediaQueryList | null {
  return typeof window !== 'undefined' && typeof window.matchMedia === 'function' ? window.matchMedia(DARK_QUERY) : null
}

/** The light or dark look to show for a theme choice. */
export function resolveTheme(mode: ThemeMode, systemDark: boolean): 'light' | 'dark' {
  if (mode === 'light' || mode === 'dark') return mode
  return systemDark ? 'dark' : 'light'
}

/** Sets <html data-theme data-palette data-design>. Unknown values fall back to System, Carbon and Classic. */
export function applyTheme(
  mode: ThemeMode | undefined,
  palette: Palette | undefined,
  design?: Design,
  root: HTMLElement = document.documentElement,
): void {
  const m: ThemeMode = mode && THEMES.includes(mode) ? mode : 'system'
  const p: Palette = palette && PALETTES.includes(palette) ? palette : 'carbon'
  const d: Design = design && DESIGNS.includes(design) ? design : 'classic'
  current = { mode: m, palette: p, design: d }
  root.dataset.theme = resolveTheme(m, systemQuery()?.matches ?? false)
  root.dataset.palette = p
  root.dataset.design = d
}

/** Re-applies the last choice whenever Windows switches light/dark. Returns a disposer. */
export function followSystemTheme(root: HTMLElement = document.documentElement): () => void {
  const q = systemQuery()
  if (!q) return () => {}
  const onChange = () => {
    if (current.mode === 'system') applyTheme(current.mode, current.palette, current.design, root)
  }
  q.addEventListener('change', onChange)
  return () => q.removeEventListener('change', onChange)
}
