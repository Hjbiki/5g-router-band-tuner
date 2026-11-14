import type { AppSnapshot } from './bindings/AppSnapshot'
import type { Settings } from './bindings/Settings'
import type { NetConfig } from './bindings/NetConfig'
import { api, onAlert, onSnapshot } from './backend'
import { ensureScan, scanActive } from './scan.svelte'
import { resolveLanguage, setLanguage } from './i18n'
import { alertToToast } from './alerts'
import { routerKey, watchPrivacyLogin } from './privacyCheck.svelte'

export type Sample = {
  t: number
  lteSinr: number | null
  lteRsrp: number | null
  nrSinr: number | null
  nrRsrp: number | null
}
export type Toast = { id: number; kind: 'info' | 'error'; text: string }

/** 300 samples at 1 per second is about 5 minutes. */
export const HISTORY_CAP = 300
export const MAX_TOASTS = 5

export type Tab = 'dashboard' | 'bands' | 'scan' | 'speedtest' | 'router' | 'history' | 'settings' | 'about'

/**
 * Where the History page should take the user when another page sends them there: the answer card,
 * the newest band change in the event log, or the speed by bands or by IP range tables. A section
 * that is closed opens (and stays open). The page clears it once handled.
 */
export type HistoryFocus = null | 'answer' | 'latestChange' | 'bandSets' | 'ipRanges'

export const app: {
  tab: Tab
  snapshot: AppSnapshot | null
  settings: Settings | null
  history: Sample[]
  toasts: Toast[]
  historyFocus: HistoryFocus
} = $state({ tab: 'dashboard', snapshot: null, settings: null, history: [], toasts: [], historyFocus: null })

/**
 * Unsaved band edits. Kept here, not in the Bands panel, so switching tabs doesn't discard
 * them. `dirty` false means the draft follows the router; `wasClosed` tracks whether the last
 * seen change had finished, so a change that ends while the panel is closed still resets it.
 * `primaryEdited` means a star moved since the last Apply, which is a change to apply even with the
 * same bands.
 */
export const bandEdit: { draft: NetConfig | null; dirty: boolean; wasClosed: boolean; primaryEdited: boolean } = $state({
  draft: null,
  dirty: false,
  wasClosed: true,
  primaryEdited: false,
})

let nextToast = 1

/** Whether `tryBands` has a band config to start from. */
export function canTryBands(): boolean {
  return Boolean(bandEdit.draft ?? app.snapshot?.config)
}

/**
 * Puts a band set (Huawei hex) in the band editor and opens the Bands page. Nothing is sent to the
 * router: the user still reviews and applies it there. False when there's no config to start from.
 */
export function tryBands(lteBands: string): boolean {
  const base = bandEdit.draft ?? app.snapshot?.config
  if (!base) return false
  bandEdit.draft = { ...($state.snapshot(base) as NetConfig), lteBands }
  bandEdit.dirty = true
  app.tab = 'bands'
  return true
}

export function ingestSnapshot(s: AppSnapshot): void {
  app.snapshot = s
  // Here and not in the card, so a login that ends on another page is noticed too.
  watchPrivacyLogin(s.session.kind, routerKey(app.settings))
  app.history.push({
    t: s.capturedAtMs,
    lteSinr: s.signal?.lte?.sinr ?? null,
    lteRsrp: s.signal?.lte?.rsrp ?? null,
    nrSinr: s.signal?.nr?.sinr ?? null,
    nrRsrp: s.signal?.nr?.rsrp ?? null,
  })
  if (app.history.length > HISTORY_CAP) app.history.splice(0, app.history.length - HISTORY_CAP)
}

export function pushToast(text: string, kind: Toast['kind'] = 'info'): number {
  const id = nextToast++
  app.toasts.push({ id, kind, text })
  // Over the cap, the oldest info toast goes first; errors stay until dismissed if possible.
  while (app.toasts.length > MAX_TOASTS) {
    const info = app.toasts.findIndex((x) => x.kind === 'info')
    app.toasts.splice(info >= 0 ? info : 0, 1)
  }
  return id
}

export function dismissToast(id: number): void {
  const i = app.toasts.findIndex((x) => x.id === id)
  if (i >= 0) app.toasts.splice(i, 1)
}

let settingsChain: Promise<unknown> = Promise.resolve()

/**
 * Saves a settings change and then updates the store. Saves run one at a time, and each
 * `change` gets the newest stored settings when its turn comes, so two writes from different
 * places (a star and a preset, say) can't overwrite each other. Resolves with what was saved,
 * or null when settings aren't loaded; a refused save rejects and leaves the store unchanged.
 */
export function updateSettings(change: (s: Settings) => Settings): Promise<Settings | null> {
  const run = async (): Promise<Settings | null> => {
    if (!app.settings) return null
    const next = change($state.snapshot(app.settings) as Settings)
    await api.saveSettings(next)
    app.settings = next
    // A save that names another router may not be followed by a snapshot.
    watchPrivacyLogin(app.snapshot?.session.kind, routerKey(next))
    return next
  }
  const result = settingsChain.then(run, run)
  settingsChain = result.catch(() => {})
  return result
}

/**
 * Applies the fields that differ between `base` (what a form started from) and `edited`
 * onto `current`. Nested objects are compared field by field; arrays and values whole.
 * Fields the user didn't touch keep whatever `current` has, even if it changed meanwhile.
 */
export function mergeEdits<T>(base: T, edited: T, current: T): T {
  const plain = (v: unknown): v is Record<string, unknown> =>
    v !== null && typeof v === 'object' && !Array.isArray(v)
  if (plain(base) && plain(edited) && plain(current)) {
    const out: Record<string, unknown> = { ...current }
    for (const k of Object.keys(edited)) out[k] = mergeEdits(base[k], edited[k], current[k])
    return out as T
  }
  return JSON.stringify(base) === JSON.stringify(edited) ? current : edited
}

/** Loads settings and the first snapshot, then follows backend events. Returns a disposer. */
export async function startStore(): Promise<() => void> {
  app.settings = await api.getSettings()
  setLanguage(resolveLanguage(app.settings.language))
  ingestSnapshot(await api.getSnapshot())
  const offSnap = await onSnapshot(ingestSnapshot)
  // The OS notification is sent by the backend; here alerts only become in-app toasts.
  void ensureScan()
  const offAlert = await onAlert((a) => {
    // A scan changes bands on purpose, and some of those changes are meant to fail.
    if (scanActive() && (a.kind === 'safeApply' || a.kind === 'cellChanged')) return
    const toast = alertToToast(a)
    if (toast) pushToast(toast.text, toast.kind)
  })
  return () => {
    offSnap()
    offAlert()
  }
}
