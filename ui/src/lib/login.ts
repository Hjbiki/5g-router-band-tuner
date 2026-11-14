import type { CredentialStatus } from './bindings/CredentialStatus'
import type { SessionView } from './bindings/SessionView'
import type { MessageKey } from './i18n'
import type { AppSnapshot } from './bindings/AppSnapshot'
import type { Settings } from './bindings/Settings'
import { changeOpen } from './bands'

/**
 * What the login asks of the user, merged from the live session and whether a password is
 * saved. Each state has one obvious action (see LoginPanel.svelte).
 */
export type LoginState =
  | { kind: 'loading' }
  | { kind: 'connected' }
  | { kind: 'working'; why: 'connecting' | 'retrying' | 'takenOver'; retryInS?: number }
  | { kind: 'needPassword' }
  | { kind: 'wrongPassword'; code?: number }
  | { kind: 'locked'; waitS: number }
  | { kind: 'confirmRouter'; firstTime: boolean }
  | { kind: 'ready'; why: 'saved' | 'token'; code?: number }
  | { kind: 'unsupported' }
  /** The user logged out; nothing reaches the router until Connect. */
  | { kind: 'loggedOut' }

/**
 * Nobody is logged in and nothing is on its way: pages say "log in first" instead of waiting for
 * data. Connecting, reconnecting and a taken-over session resolve by themselves.
 */
export function needsLogin(session: SessionView | null | undefined): boolean {
  if (!session) return false
  return !['loggedIn', 'connecting', 'reconnecting', 'takenOver'].includes(session.kind)
}

/** 125001 to 125003 refuse the CSRF token before any password check (see `password_marker`). */
export function isTokenCode(code: number): boolean {
  return code >= 125001 && code <= 125003
}

export function loginState(session: SessionView | null, cred: CredentialStatus | null): LoginState {
  if (!session) return { kind: 'loading' }
  switch (session.kind) {
    case 'loggedIn':
      return { kind: 'connected' }
    case 'connecting':
      return { kind: 'working', why: 'connecting' }
    case 'reconnecting':
      return { kind: 'working', why: 'retrying', retryInS: session.retryInS }
    case 'takenOver':
      return { kind: 'working', why: 'takenOver' }
    // Straight from the session: the marker may not be on file yet when this snapshot arrives.
    case 'badCredentials':
      return { kind: 'wrongPassword' }
    case 'loginRejected':
      return isTokenCode(session.code)
        ? { kind: 'ready', why: 'token', code: session.code }
        : { kind: 'wrongPassword', code: session.code }
    case 'lockedOut':
      return { kind: 'locked', waitS: session.waitS }
    case 'algorithmMismatch':
      return { kind: 'unsupported' }
    case 'confirmRouter':
      return { kind: 'confirmRouter', firstTime: session.firstTime }
    case 'loggedOut':
      return { kind: 'loggedOut' }
    case 'notConfigured':
      if (!cred) return { kind: 'loading' }
      // Rejected in an earlier run: launch didn't try it, and it must be typed again.
      if (cred.rejected) return { kind: 'wrongPassword' }
      if (!cred.saved) return { kind: 'needPassword' }
      return { kind: 'ready', why: 'saved' }
  }
}

/** A few words for the sidebar status and toasts. */
export function loginShort(s: LoginState): MessageKey {
  switch (s.kind) {
    case 'loading':
      return 'app.loading'
    case 'connected':
      return 'login.short.connected'
    case 'working':
      return 'login.short.working'
    case 'needPassword':
      return 'login.short.needPassword'
    case 'wrongPassword':
      return 'login.short.wrong'
    case 'locked':
      return 'login.short.locked'
    case 'confirmRouter':
      return 'login.short.confirm'
    case 'ready':
      return 'login.short.ready'
    case 'unsupported':
      return 'login.short.unsupported'
    case 'loggedOut':
      return 'login.short.loggedOut'
  }
}

/** How the state reads at a glance, for the sidebar dot. */
export function loginTone(s: LoginState): 'ok' | 'warn' | 'bad' {
  switch (s.kind) {
    case 'connected':
      return 'ok'
    case 'wrongPassword':
    case 'locked':
    case 'unsupported':
      return 'bad'
    default:
      return 'warn'
  }
}

/** States that wait for the user; the app-wide banner shows these on every page. */
export function needsAttention(s: LoginState): boolean {
  switch (s.kind) {
    case 'needPassword':
    case 'wrongPassword':
    case 'locked':
    case 'confirmRouter':
    case 'ready':
    case 'unsupported':
    case 'loggedOut':
      return true
    default:
      return false
  }
}

/**
 * Whether the first-run guide is on screen. Never over an active change: the safe-apply banner's
 * Keep and Revert must stay reachable.
 */
export function onboardingOpen(settings: Settings | null, snapshot: AppSnapshot | null): boolean {
  return !!settings && !settings.onboardingDone && !!snapshot && !changeOpen(snapshot.safeApply)
}
