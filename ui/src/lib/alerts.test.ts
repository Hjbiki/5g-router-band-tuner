import { describe, expect, it } from 'vitest'
import { alertToToast, rejectedText, sessionText } from './alerts'
import { t } from './i18n'

describe('alerts', () => {
  it('a revert that ended on Auto says so', () => {
    const toast = alertToToast({
      kind: 'safeApply',
      event: { kind: 'reverted', reason: 'noReattach', fallback: true },
    })
    expect(toast?.text).toBe(t('safeApply.event.revertedToAuto'))
    const normal = alertToToast({
      kind: 'safeApply',
      event: { kind: 'reverted', reason: 'noReattach', fallback: false },
    })
    expect(normal?.text).toBe(t('safeApply.event.reverted'))
  })

  it('known rejections are translated', () => {
    expect(rejectedText('not connected to the router')).toBe(t('safeApply.rejected.notConnected'))
    expect(rejectedText('another change is in progress')).toBe(t('safeApply.rejected.inProgress'))
    const toast = alertToToast({ kind: 'safeApply', event: { kind: 'rejected', message: 'no router session' } })
    expect(toast).toEqual({ kind: 'error', text: t('safeApply.rejected.notConnected') })
  })

  it('a refusal because a speed test runs is translated', () => {
    expect(rejectedText('a speed test is running')).toBe(t('safeApply.rejected.speedTestRunning'))
  })

  it('unknown rejections keep the raw text behind a localized prefix', () => {
    const raw = 'could not read the current configuration: router API error 100004 (Busy)'
    expect(rejectedText(raw)).toBe(t('safeApply.routerError', { message: raw }))
    expect(rejectedText(raw)).toContain('100004')
  })

  it('login rejected shows the router code', () => {
    const text = sessionText({ kind: 'loginRejected', code: 108009 })
    expect(text).toContain('108009')
    expect(text).toBe(t('session.loginRejected', { code: '⁦108009⁩' }))
  })

  it('a refused CSRF token is not blamed on the password', () => {
    const text = sessionText({ kind: 'loginRejected', code: 125003 })
    expect(text).toBe(t('session.loginRejectedToken', { code: '⁦125003⁩' }))
    expect(text).not.toBe(t('session.loginRejected', { code: '⁦125003⁩' }))
  })

  it('login stops become short error toasts; progress and the rest are left to the login panel', () => {
    const toast = (view: object) => alertToToast({ kind: 'session', view } as never)
    expect(toast({ kind: 'badCredentials' })).toEqual({ kind: 'error', text: t('login.short.wrong') })
    expect(toast({ kind: 'lockedOut', waitS: 60 })).toEqual({ kind: 'error', text: t('login.short.locked') })
    expect(toast({ kind: 'confirmRouter', firstTime: true })).toEqual({ kind: 'error', text: t('login.short.confirm') })
    expect(toast({ kind: 'loginRejected', code: 125003 })).toBeNull()
    expect(toast({ kind: 'takenOver' })).toBeNull()
    expect(toast({ kind: 'connecting' })).toBeNull()
    expect(toast({ kind: 'loggedIn' })).toBeNull()
    // The Log out button shows its own toast.
    expect(toast({ kind: 'loggedOut' })).toBeNull()
  })

  it('names the logged-out session', () => {
    expect(sessionText({ kind: 'loggedOut' })).toBe(t('session.loggedOut'))
  })

  it('automation events become toasts; a failed step 1 leaves it to the revert toast', () => {
    const ev = (event: object) => alertToToast({ kind: 'automation', event } as never)
    expect(ev({ kind: 'lossIgnored', reason: 'lossOfService' })).toEqual({ kind: 'error', text: t('notify.lossIgnored') })
    expect(ev({ kind: 'primaryLockFailed', primary: 3, origin: 'user' })).toBeNull()
    expect(ev({ kind: 'relockStarted', primary: 3, servingBand: 1 })?.text).toContain('B3')
    expect(ev({ kind: 'relockGaveUp', primary: 3, attempts: 3 })?.kind).toBe('error')
    expect(ev({ kind: 'widenFailed', primary: 3, origin: 'relock', endedOn: 'auto' })?.text).toBe(t('safeApply.revertedToAuto'))
    expect(ev({ kind: 'widenFailed', primary: 3, origin: 'user', endedOn: 'original' })?.text).toContain('B3')
    expect(ev({ kind: 'sequenceResumed', primary: 3, origin: 'user', step: 2 })?.text).toContain('B3')
    expect(ev({ kind: 'sequenceDiscarded' })?.kind).toBe('error')
  })

  it('an APN switch toasts how it ended, never its steps or refusals', () => {
    const ev = (event: object) => alertToToast({ kind: 'apn', event } as never)
    expect(ev({ kind: 'started' })).toBeNull()
    expect(ev({ kind: 'reattached' })).toBeNull()
    expect(ev({ kind: 'rejected', message: 'x' })).toBeNull()
    expect(ev({ kind: 'committed' })).toEqual({ kind: 'info', text: t('safeApply.apn.event.committed') })
    expect(ev({ kind: 'reverted', reason: 'noReattach', fallback: false })?.text).toBe(t('safeApply.apn.event.reverted'))
    expect(ev({ kind: 'reverted', reason: 'applyRejected', fallback: false })?.kind).toBe('error')
    expect(ev({ kind: 'revertFailed' })).toEqual({ kind: 'error', text: t('safeApply.apn.event.revertFailed') })
  })

  it('a band switch for a new IP toasts how it ended, and names no band when step 2 fails', () => {
    const ended = (outcome: string) =>
      alertToToast({ kind: 'ipReattachEnded', result: { outcome, from: '10.45', to: '10.62' } } as never)
    expect(ended('changed')).toEqual({
      kind: 'info',
      text: t('ipReattach.toastEnded', { outcome: t('ipReattach.outcome.changed') }),
    })
    expect(ended('step2Failed')?.kind).toBe('error')
    expect(ended('reverted')?.kind).toBe('info')
    const widen = alertToToast({
      kind: 'automation',
      event: { kind: 'widenFailed', primary: 1, origin: 'ipReattach', endedOn: 'primaryOnly' },
    })
    expect(widen).toEqual({ kind: 'error', text: t('notify.widenFailedReattach') })
    const resumed = alertToToast({
      kind: 'automation',
      event: { kind: 'sequenceResumed', primary: 1, origin: 'ipReattach', step: 1 },
    })
    expect(resumed).toEqual({ kind: 'info', text: t('notify.reattachResumed') })
  })

  it('an unexpected APN switch after a profile write says whether it is being undone', () => {
    expect(alertToToast({ kind: 'apnUnexpectedSwitch', reverting: true })).toEqual({
      kind: 'info',
      text: t('network.apn.edit.unexpected'),
    })
    expect(alertToToast({ kind: 'apnUnexpectedSwitch', reverting: false })).toEqual({
      kind: 'error',
      text: t('network.apn.edit.unexpectedKept'),
    })
  })

  it('a band change refused during an APN switch says why', () => {
    expect(rejectedText('an APN change is in progress')).toBe(t('safeApply.rejected.apnChangeRunning'))
  })
})
