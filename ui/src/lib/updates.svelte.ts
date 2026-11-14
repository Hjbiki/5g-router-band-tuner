import type { UpdateView } from './bindings/UpdateView'
import { api, onUpdate } from './backend'

export const updates: { view: UpdateView | null } = $state({ view: null })

let started: Promise<void> | null = null

/** Loads what's known, with no network call, and follows later checks. */
export function ensureUpdates(): Promise<void> {
  started ??= (async () => {
    await onUpdate((v) => (updates.view = v))
    updates.view = await api.updateStatus()
  })().catch(() => {
    started = null
  })
  return started
}

/** For tests. */
export function resetUpdates(): void {
  started = null
  updates.view = null
}
