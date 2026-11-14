// The privacy check's last result, kept in memory while the app runs, so that leaving the Router
// page and coming back doesn't lose it. It is never written to disk: closing the app forgets it,
// and so does the end of the login it was read in, on whatever page that happens.
import type { DnsCheck } from './bindings/DnsCheck'
import type { PrivacyReport } from './bindings/PrivacyReport'
import type { SessionView } from './bindings/SessionView'
import type { Settings } from './bindings/Settings'

export const privacy = $state({
  report: null as PrivacyReport | null,
  /** A check is reading the router. Here and not in the card, so leaving the page can't start a second one. */
  running: false,
  /** Device names and addresses are on screen (the user asked for them). */
  showDevices: false,
  dns: null as DnsCheck | null,
  showResolver: false,
})

// The router the kept report was read from (`routerKey`).
let readFrom = ''
// Goes up whenever a login ends or the settings name another router, whether or not a report is
// kept, so a check that was still reading then can tell its answer is from before.
let generation = 0
let lastRouter: string | null = null

/** A check that has started: what was true when it did. */
export type PrivacyRun = { generation: number; router: string }

/** The router the settings name: its driver and address. */
export function routerKey(settings: Pick<Settings, 'host' | 'driver'> | null): string {
  return settings ? `${settings.driver}|${settings.host}` : ''
}

/** A login that is still the one a report was read in: logged in, or on its way back. */
export function sameLogin(kind: SessionView['kind'] | null | undefined): boolean {
  return kind === 'loggedIn' || kind === 'connecting' || kind === 'reconnecting'
}

/** Call before asking the router, with the router the settings name then. */
export function beginPrivacyRun(router: string): PrivacyRun {
  return { generation, router }
}

/**
 * Keeps a report until its login ends. Dropped (false) when the login ended or the router changed
 * while it was being read, even if a new login has begun since: it belongs to the old one.
 */
export function keepPrivacyReport(report: PrivacyReport, run: PrivacyRun, routerNow: string): boolean {
  if (run.generation !== generation || run.router !== routerNow) return false
  privacy.report = report
  readFrom = run.router
  return true
}

/** Drops what was read from the router. The DNS answer isn't the router's, so it stays. */
export function forgetPrivacyCheck(): void {
  privacy.report = null
  privacy.showDevices = false
}

/**
 * Called for every snapshot and every saved settings change, whatever page is open. A short
 * reconnect keeps the report; a logout, a refused password or settings that name another router
 * forget it, and mark any check still running as out of date.
 */
export function watchPrivacyLogin(kind: SessionView['kind'] | null | undefined, router: string): void {
  const ended = !sameLogin(kind)
  if (ended || (lastRouter !== null && router !== lastRouter)) generation += 1
  lastRouter = router
  if (privacy.report && (ended || router !== readFrom)) forgetPrivacyCheck()
}
