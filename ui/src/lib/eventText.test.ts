import { afterEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))

import { setLanguage, t, type MessageKey } from './i18n'
import { bandList, changeSeverity, changeTitle, csvChangeTemplate, renderEventText, translate } from './eventText'
import type { ChangeGroup } from './bindings/ChangeGroup'

const LRI = '⁦'
const PDI = '⁩'

const group = (o: Partial<ChangeGroup> = {}): ChangeGroup => ({
  startedMs: 0,
  endedMs: 1,
  from: [1, 3],
  to: [3],
  primary: null,
  outcome: { kind: 'kept' },
  steps: [],
  ...o,
})

describe('eventText', () => {
  afterEach(() => setLanguage('en'))

  it('puts each parameter in words: bands, numbers, keys and text', () => {
    expect(renderEventText('events.kind.sequenceStarted', { primary: { t: 'number', v: 3 }, bands: { t: 'bands', v: [1, 3] } }, 'western')).toBe(
      `Two-step change started: B3 first, then ${LRI}B1 + B3${PDI}`,
    )
    expect(renderEventText('events.kind.safeApplyReverted', { reason: { t: 'key', v: 'safeApply.reason.connectionLost' } }, 'western')).toBe(
      `Went back to the earlier bands: ${t('safeApply.reason.connectionLost')}`,
    )
    expect(renderEventText('events.kind.cellChangedSameBand', { band: { t: 'text', v: 'B3' } }, 'western')).toBe(
      `Moved to another ${LRI}B3${PDI} cell`,
    )
    expect(bandList([41, 78], 'western', 'n')).toBe(`${LRI}n41 + n78${PDI}`)
  })

  it('writes Arabic with Arabic-Indic digits and keeps the bands left to right', () => {
    setLanguage('ar')
    expect(renderEventText('events.kind.sequenceStep2', { bands: { t: 'bands', v: [1, 3] } }, 'arabicIndic')).toBe(
      `الخطوة الثانية: السماح بـ ${LRI}B١ + B٣${PDI}`,
    )
    expect(changeTitle(group(), 'arabicIndic')).toBe(`تغيّرت النطاقات إلى ${LRI}B٣${PDI} وبقيت (كانت ${LRI}B١ + B٣${PDI})`)
  })

  it('shows a key it has no text for as the key, instead of failing', () => {
    expect(translate('events.kind.fromTheFuture')).toBe('events.kind.fromTheFuture')
    expect(renderEventText('events.kind.unknown', { code: { t: 'text', v: 'odd.kind' } }, 'western')).toContain('odd.kind')
  })

  it('names a change by how it ended, with "other bands" when the log lacks them', () => {
    expect(changeTitle(group({ from: [], to: [], outcome: { kind: 'rejected' } }), 'western')).toBe(
      t('events.change.rejected', { to: t('history.marker.otherBands'), from: t('history.marker.otherBands') }),
    )
    expect(changeSeverity({ kind: 'kept' })).toBe('good')
    expect(changeSeverity({ kind: 'reverted', reason: 'x', fallback: false })).toBe('warning')
    expect(changeSeverity({ kind: 'rejected' })).toBe('warning')
    expect(changeSeverity({ kind: 'revertFailed' })).toBe('problem')
    expect(changeSeverity({ kind: 'open' })).toBe('info')
  })

  it('gives the CSV one sentence for an undone change, reason included', () => {
    expect(csvChangeTemplate('reverted')).toBe('Tried {to}, then went back to {from}. Why: {reason}')
    expect(csvChangeTemplate('kept')).toBe(t('events.change.kept'))
  })

  it("names the router's own code when the backend sends one", () => {
    const code = { t: 'number', v: 100003 } as const
    expect(renderEventText('events.kind.sessionLostCode', { code }, 'western')).toBe('Lost the router session (router code 100003)')
    expect(renderEventText('events.kind.featureRefusedCode', { what: { t: 'text', v: 'apn' }, code }, 'western')).toContain(
      'router code 100003',
    )
    expect(
      renderEventText('events.kind.ipRefreshEndedCode', { outcome: { t: 'key', v: 'ipRefresh.outcome.offRefused' }, code }, 'western'),
    ).toContain('(router code 100003)')
    expect(renderEventText('events.kind.ipRefreshMethodFallbackCode', { code }, 'western')).toContain('(router code 100003)')
    expect(
      renderEventText('events.kind.ipRefreshSearchFailedCode', { n: { t: 'number', v: 1 }, seconds: { t: 'number', v: 2 }, code }, 'western'),
    ).toContain('(router code 100003)')
    setLanguage('ar')
    expect(renderEventText('events.kind.sessionLostCode', { code }, 'arabicIndic')).toBe('انتهت جلسة الراوتر (رمز الراوتر ١٠٠٠٠٣)')
  })

  it('has text for every key the backend describer writes', () => {
    const kinds = [
      'safeApplyStarted', 'safeApplyApplied', 'safeApplyVerified', 'safeApplyReattached', 'safeApplyCommitted',
      'safeApplyReverted', 'safeApplyFellBackToAuto', 'safeApplyRevertFailed', 'safeApplyRejected', 'safeApplyResumed',
      'safeApplyLossIgnored', 'safeApplyRead', 'safeApplyReadFailed', 'safeApplyPost', 'safeApplyPostFailed',
      'journalForeign', 'journalUnreadable', 'journalWriteFailed', 'journalArchiveFailed',
      'sequenceStarted', 'sequenceStep2', 'sequenceEnded', 'sequenceResumed', 'sequenceDiscarded', 'sequenceCancelled',
      'sequenceWriteFailed', 'sequenceClearFailed', 'relockStarted', 'relockSucceeded', 'relockFailed', 'relockGaveUp',
      'relockReset', 'automationSettings', 'speedtestStarted', 'speedtestStopped', 'speedtestFailed', 'speedtestAutoSkipped',
      'scanStarted', 'scanStopped', 'deviceReboot', 'deviceFactoryReset', 'sessionLoggedIn', 'sessionConnecting',
      'sessionNotConfigured', 'sessionConfirmRouter', 'sessionReconnecting', 'sessionTakenOver', 'sessionBadCredentials',
      'sessionLockedOut', 'sessionAlgorithmMismatch', 'sessionLoginRejected', 'sessionLost', 'sessionLostCode', 'sessionRefreshed',
      'featureRefused', 'featureRefusedCode', 'ipRefreshEndedCode',
      'sessionIdentityNotSaved', 'sessionMarkerNotSaved', 'sessionLoggedOut', 'sessionLogout', 'sessionLogoutUnconfirmed',
      'sessionLogoutMarkerNotSaved', 'sessionAutoConnectSkipped', 'sessionLogoutOverridden', 'heartbeatFailed', 'cellChanged', 'cellChangedSameBand', 'wanRangeChanged', 'notify',
      'dataJournalForeign', 'dataJournalUnreadable', 'ipRefreshStarted', 'ipRefreshStartedTarget', 'ipRefreshTry',
      'ipRefreshTryNoRange', 'ipRefreshEnded', 'ipRefreshResumed', 'ipRefreshDataOnFailed', 'ipRefreshManualOn',
      'ipRefreshManualOnFailed', 'ipRefreshTrySearch', 'ipRefreshTrySearchNoRange', 'ipRefreshSearch',
      'ipRefreshSearchNoCount', 'ipRefreshSearchTimedOut', 'ipRefreshSearchFailed', 'ipRefreshSearchFailedCode',
      'ipRefreshMethodFallback', 'ipRefreshMethodFallbackCode', 'ipRefreshBandsChanged', 'ipReattachStarted',
      'ipReattachStep1Kept', 'ipReattachEnded', 'ipReattachEndedTo', 'apnStarted', 'apnApplied', 'apnVerified', 'apnReattached', 'apnCommitted',
      'apnReverted', 'apnRevertFailed', 'apnRejected', 'apnResumed', 'apnProfileAdded', 'apnProfileEdited',
      'apnProfileDeleted', 'apnUnexpectedSwitch', 'apnUnexpectedSwitchKept',
      'alertsLagged', 'appStarted', 'appStopped', 'historyPruned', 'historyCleared', 'historyReset', 'historyImported',
      'historyDropped', 'historyExported', 'diagnosticsPageScriptsSaved', 'privacyChecked', 'unknown',
    ]
    const why = [
      'fellBackToAuto', 'revertFailed', 'lossIgnored', 'foreignJournal', 'journalUnreadable', 'sequenceDiscarded',
      'relockGaveUp', 'sessionLost', 'historyReset', 'offline', 'loggedOut', 'bandChange', 'autoScan', 'speedTest',
      'dataJournalBlocked', 'ipRefreshResumed', 'dataOnFailed', 'ipRefresh', 'apnRevertFailed', 'apnChange',
      'apnUnexpectedSwitch', 'apnUnexpectedSwitchKept', 'ipRefreshBandsChanged',
    ]
    const keys = [
      ...kinds.map((k) => `events.kind.${k}`),
      ...why.map((k) => `events.why.${k}`),
      ...['open', 'kept', 'reverted', 'revertFailed', 'rejected'].map((k) => `events.change.${k}`),
      ...['changed', 'unchanged', 'unknown', 'step1Failed', 'step2Failed', 'reverted', 'cancelled'].map(
        (k) => `ipReattach.outcome.${k}`,
      ),
      ...['info', 'good', 'warning', 'problem'].map((k) => `events.severity.${k}`),
      'events.value.on',
      'events.value.off',
      'safeApply.reason.unknown',
    ]
    for (const lang of ['en', 'ar']) {
      setLanguage(lang)
      for (const k of keys) expect(t(k as MessageKey), `${lang} ${k}`).not.toBe(k)
    }
  })
})
