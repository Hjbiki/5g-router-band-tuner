import type { Alert } from './bindings/Alert'
import type { CellKey } from './bindings/CellKey'
import type { SessionView } from './bindings/SessionView'
import { t, type MessageKey } from './i18n'
import { formatNumber } from './format'
import { app, type Toast } from './store.svelte'
import { loginShort, loginState } from './login'
import { outcomeIsTrouble, outcomeKey } from './ipRefresh.svelte'

/** Localised number isolated as LTR (same effect as <bdi dir="ltr"> inside a plain string). */
function isoNum(n: number | null): string {
  return `⁦${formatNumber(n, { digits: app.settings?.digits })}⁩`
}

export function cellLabel(c: CellKey): string {
  return `${c.band || '—'} · ${t('dashboard.pci')} ${isoNum(c.pci)}`
}

export function sessionText(v: SessionView): string {
  switch (v.kind) {
    case 'notConfigured': return t('session.notConfigured')
    case 'connecting': return t('session.connecting')
    case 'loggedIn': return t('session.loggedIn')
    case 'reconnecting': return t('session.reconnecting', { seconds: isoNum(v.retryInS) })
    case 'takenOver': return t('session.takenOver')
    case 'badCredentials': return t('session.badCredentials')
    case 'lockedOut': return t('session.lockedOut', { seconds: isoNum(v.waitS) })
    case 'algorithmMismatch': return t('session.algorithmMismatch')
    case 'loginRejected':
      // 125001–125003: CSRF token refused before any password check.
      return v.code >= 125001 && v.code <= 125003
        ? t('session.loginRejectedToken', { code: isoNum(v.code) })
        : t('session.loginRejected', { code: isoNum(v.code) })
    case 'confirmRouter': return t('session.confirmRouter')
    case 'loggedOut': return t('session.loggedOut')
  }
}

/** Safe-apply refusal strings (Rust) mapped to dictionary keys. */
const REJECTED: Record<string, MessageKey> = {
  'a pending change for another router is on file': 'safeApply.rejected.foreignJournal',
  "the pending-change journal can't be read": 'safeApply.rejected.journalUnreadable',
  'not connected to the router': 'safeApply.rejected.notConnected',
  'no router session': 'safeApply.rejected.notConnected',
  "the router's capabilities aren't known yet": 'safeApply.rejected.capabilitiesUnknown',
  'another change is in progress': 'safeApply.rejected.inProgress',
  "band editing isn't allowed on this router right now": 'safeApply.rejected.editNotAllowed',
  'select at least one LTE band': 'safeApply.rejected.noBands',
  "network mode isn't supported by this router": 'safeApply.rejected.modeUnsupported',
  "5G option isn't supported by this router": 'safeApply.rejected.nrOptionUnsupported',
  'the requested configuration is already active': 'safeApply.rejected.alreadyActive',
  'the router already has the requested configuration': 'safeApply.rejected.alreadyActive',
  'a speed test is running': 'safeApply.rejected.speedTestRunning',
  'an APN change is in progress': 'safeApply.rejected.apnChangeRunning',
  'an IP refresh is running': 'ipRefresh.refuse.running',
}

/** A `Rejected` message in the UI language; unknown ones keep the raw text behind "Router error:". */
export function rejectedText(message: string): string {
  const key = REJECTED[message]
  return key ? t(key) : t('safeApply.routerError', { message })
}

/** Login stops that wait for the user; the login panel explains them, the toast only names them. */
const LOGIN_FAILURES = new Set<SessionView['kind']>(['badCredentials', 'lockedOut', 'confirmRouter'])

/** Maps a backend alert to toast content; null means nothing to show. */
export function alertToToast(a: Alert): Pick<Toast, 'kind' | 'text'> | null {
  switch (a.kind) {
    case 'cellChanged':
      return { kind: 'info', text: t('dashboard.cellChanged', { from: cellLabel(a.from), to: cellLabel(a.to) }) }
    case 'safeApply': {
      const e = a.event
      const bad = e.kind === 'revertFailed' || e.kind === 'rejected' || e.kind === 'fellBackToAuto'
      const text =
        e.kind === 'rejected'
          ? rejectedText(e.message)
          : e.kind === 'reverted' && e.fallback
            ? t('safeApply.event.revertedToAuto')
            : t(`safeApply.event.${e.kind}`)
      return { kind: bad ? 'error' : 'info', text }
    }
    case 'session':
      // Login stops stay on screen until dismissed (error toasts don't time out), with a short
      // title: the login panel above every page carries the explanation and the action.
      return LOGIN_FAILURES.has(a.view.kind) ? { kind: 'error', text: t(loginShort(loginState(a.view, null))) } : null
    case 'automation': {
      // Primary band / re-lock / loss-policy events. The OS notification comes from the backend;
      // a failed step 1 is already reported by its own safe-apply revert.
      const e = a.event
      const band = (n: number) => `⁦B${formatNumber(n, { digits: app.settings?.digits })}⁩`
      switch (e.kind) {
        case 'lossIgnored':
          return { kind: 'error', text: t('notify.lossIgnored') }
        case 'primaryLockFailed':
          return null
        case 'widenFailed':
          return {
            kind: 'error',
            text:
              e.endedOn === 'auto'
                ? t('safeApply.revertedToAuto')
                : e.origin === 'ipReattach'
                  ? // Step 1 of a band switch for a new IP isn't one band, so it isn't named.
                    t('notify.widenFailedReattach')
                  : t(e.endedOn === 'original' ? 'notify.widenFailedOriginal' : 'notify.widenFailedPrimary', {
                      band: band(e.primary),
                    }),
          }
        case 'relockStarted':
          return { kind: 'info', text: t('notify.relockStarted', { band: band(e.primary) }) }
        case 'relockGaveUp':
          return { kind: 'error', text: t('notify.relockGaveUp', { band: band(e.primary) }) }
        case 'sequenceResumed':
          if (e.origin === 'ipReattach') return { kind: 'info', text: t('notify.reattachResumed') }
          return { kind: 'info', text: t('notify.sequenceResumed', { band: band(e.primary) }) }
        case 'sequenceDiscarded':
          return { kind: 'error', text: t('notify.sequenceDiscarded') }
      }
      return null
    }
    case 'ipRefresh': {
      // The banner shows progress; the outcome and a stuck "on" also get a toast.
      const e = a.event
      if (e.kind === 'dataOnFailed') return { kind: 'error', text: t('notify.dataOnFailed') }
      const outcome = t(outcomeKey(e.result.outcome, e.result.method))
      return { kind: outcomeIsTrouble(e.result.outcome) ? 'error' : 'info', text: t('ipRefresh.banner.ended', { outcome }) }
    }
    case 'apn': {
      // The APN banner shows the steps; only how it ended gets a toast. Refusals are the
      // command's own reply, shown in the dialog.
      const e = a.event
      switch (e.kind) {
        case 'committed':
          return { kind: 'info', text: t('safeApply.apn.event.committed') }
        case 'reverted':
          return e.reason === 'applyRejected'
            ? { kind: 'error', text: t('safeApply.apn.event.notApplied') }
            : { kind: 'info', text: t('safeApply.apn.event.reverted') }
        case 'revertFailed':
        case 'fellBackToAuto':
          return { kind: 'error', text: t('safeApply.apn.event.revertFailed') }
        default:
          return null
      }
    }
    case 'ipReattachEnded': {
      const o = a.result.outcome
      const outcome = t(`ipReattach.outcome.${o}` as MessageKey)
      const trouble = o === 'step1Failed' || o === 'step2Failed'
      return { kind: trouble ? 'error' : 'info', text: t('ipReattach.toastEnded', { outcome }) }
    }
    case 'apnUnexpectedSwitch':
      return a.reverting
        ? { kind: 'info', text: t('network.apn.edit.unexpected') }
        : { kind: 'error', text: t('network.apn.edit.unexpectedKept') }
  }
}
