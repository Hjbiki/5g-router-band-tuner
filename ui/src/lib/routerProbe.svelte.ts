import type { ProbeView } from './bindings/ProbeView'
import { api, onRouterProbe } from './backend'

/** What the app found on the router's public pages; null until it has looked, or when it needn't. */
export const routerProbe: { view: ProbeView | null } = $state({ view: null })

let started: Promise<void> | null = null

export function ensureRouterProbe(): Promise<void> {
  started ??= (async () => {
    await onRouterProbe((v) => (routerProbe.view = v))
    routerProbe.view = (await api.routerProbeStatus()) ?? null
  })().catch(() => {
    started = null
  })
  return started
}

/** For tests. */
export function resetRouterProbe(): void {
  started = null
  routerProbe.view = null
}
