import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/svelte'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))
vi.mock('../lib/backend', () => ({
  api: {
    ipRefreshStop: vi.fn(),
    dataSwitchOn: vi.fn(),
    ipReattachStart: vi.fn(),
    applyConfig: vi.fn(),
    requestReboot: vi.fn(),
    confirmReboot: vi.fn(),
  },
}))

import { api } from '../lib/backend'
import { app } from '../lib/store.svelte'
import { t } from '../lib/i18n'
import { alertToToast } from '../lib/alerts'
import { settingsFixture, snap } from '../test-fixtures'
import IpRefreshBanner from './IpRefreshBanner.svelte'
import DataSwitchNotice from './DataSwitchNotice.svelte'
import type { IpRefreshView } from '../lib/bindings/IpRefreshView'
import type { IpRefreshResult } from '../lib/bindings/IpRefreshResult'
import type { AppSnapshot } from '../lib/bindings/AppSnapshot'
import type { NetConfig } from '../lib/bindings/NetConfig'

const view = (o: Partial<IpRefreshView> = {}): IpRefreshView => ({
  active: true,
  phase: 'off',
  method: 'dataSwitch',
  tryN: 2,
  maxTries: 5,
  target: '100',
  fromRange: '10.45',
  currentRange: '10.62',
  secondsInPhase: 3,
  secondsLeft: 12,
  dataOnFailures: 0,
  stopping: false,
  resumed: false,
  last: null,
  journalBlocked: false,
  cyclesLeft: 12,
  ...o,
})

const RESULT = { method: 'dataSwitch', refusal: null, bandsChanged: false, bandsBefore: null } as const

const flush = () => new Promise((r) => setTimeout(r, 0))

describe('IpRefreshBanner', () => {
  beforeEach(() => {
    app.settings = settingsFixture()
    vi.mocked(api.ipRefreshStop).mockResolvedValue(undefined)
  })
  afterEach(() => {
    cleanup()
    app.snapshot = null
    vi.clearAllMocks()
  })

  it('stays hidden with nothing to show', () => {
    app.snapshot = snap()
    render(IpRefreshBanner)
    expect(screen.queryByTestId('ip-refresh-banner')).toBeNull()
  })

  it('shows the try, the phase and the seconds, and stops by turning data back on', async () => {
    app.snapshot = snap({ ipRefresh: view() })
    render(IpRefreshBanner)
    const progress = screen.getByTestId('ipr-progress').textContent ?? ''
    expect(progress).toContain(t('ipRefresh.banner.tryOf', { n: '⁦2⁩', total: '⁦5⁩' }))
    expect(progress).toContain(t('ipRefresh.banner.phase.off'))
    expect(progress).toContain(t('ipRefresh.banner.seconds', { n: '⁦12⁩' }))
    expect(screen.getByTestId('ip-refresh-banner').textContent).toContain('10.62.x.x')
    // The target reads as an address start, with its tail.
    expect(screen.getByTestId('ip-refresh-banner').textContent).toContain('100.x.x.x')
    const stop = screen.getByTestId('ipr-stop') as HTMLButtonElement
    expect(stop.textContent).toBe(t('ipRefresh.banner.stop'))
    await fireEvent.click(stop)
    expect(api.ipRefreshStop).toHaveBeenCalledTimes(1)
    expect(stop.disabled).toBe(true)
    expect(stop.textContent).toBe(t('ipRefresh.banner.stopping'))
  })

  it('turns urgent while data does not come back on', () => {
    app.snapshot = snap({ ipRefresh: view({ phase: 'turningOn', dataOnFailures: 2 }) })
    render(IpRefreshBanner)
    const banner = screen.getByTestId('ip-refresh-banner')
    expect(banner.getAttribute('role')).toBe('alert')
    expect(banner.textContent).toContain(t('ipRefresh.banner.onFailed'))
  })

  it('says when it turns data back on after a restart', () => {
    app.snapshot = snap({ ipRefresh: view({ resumed: true, phase: 'turningOn', currentRange: null }) })
    render(IpRefreshBanner)
    expect(screen.getByTestId('ip-refresh-banner').textContent).toContain(t('ipRefresh.banner.resumed'))
  })

  it('shows the outcome until dismissed, with the APN note after no match', async () => {
    const last = { outcome: 'notMatched', tries: 5, target: '100', from: '10.45', to: '10.62', ...RESULT } as const
    app.snapshot = snap({ ipRefresh: view({ active: false, phase: 'idle', last }) })
    render(IpRefreshBanner)
    expect(screen.getByTestId('ipr-outcome').textContent).toBe(
      t('ipRefresh.banner.ended', { outcome: t('ipRefresh.outcome.notMatched') }),
    )
    expect(screen.getByTestId('ipr-work-note')).toBeTruthy()
    await fireEvent.click(screen.getByRole('button', { name: t('ipRefresh.banner.dismiss') }))
    expect(screen.queryByTestId('ip-refresh-banner')).toBeNull()
    // A new result shows again.
    app.snapshot = snap({ ipRefresh: view({ active: false, phase: 'idle', last: { ...last, outcome: 'matched', to: '100.70' } }) })
    await flush()
    expect(screen.getByTestId('ip-refresh-banner').classList.contains('done')).toBe(true)
    expect(screen.queryByTestId('ipr-work-note')).toBeNull()
  })

  it('makes a result that needs a look stand out by more than colour', () => {
    const last = { outcome: 'onlineTimeout', tries: 1, target: null, from: '10.45', to: null, ...RESULT } as const
    app.snapshot = snap({ ipRefresh: view({ active: false, phase: 'idle', last }) })
    render(IpRefreshBanner)
    const banner = screen.getByTestId('ip-refresh-banner')
    expect(banner.getAttribute('role')).toBe('alert')
    expect(banner.classList.contains('urgent')).toBe(true)
    expect(banner.classList.contains('done')).toBe(false)
    expect(screen.getByTestId('ipr-outcome-trouble')).toBeTruthy()
  })

  it('toasts the outcome and a stuck "on"', () => {
    const result = { outcome: 'onlineTimeout', tries: 1, target: null, from: '10.45', to: null, ...RESULT } as const
    expect(alertToToast({ kind: 'ipRefresh', event: { kind: 'ended', result } })).toEqual({
      kind: 'error',
      text: t('ipRefresh.banner.ended', { outcome: t('ipRefresh.outcome.onlineTimeout') }),
    })
    expect(alertToToast({ kind: 'ipRefresh', event: { kind: 'ended', result: { ...result, outcome: 'changed' } } })?.kind).toBe('info')
    expect(alertToToast({ kind: 'ipRefresh', event: { kind: 'dataOnFailed', attempt: 1 } })).toEqual({
      kind: 'error',
      text: t('notify.dataOnFailed'),
    })
  })
})

describe('IpRefreshBanner with a network search', () => {
  const B1_B3: NetConfig = { networkMode: { kind: 'auto' }, lteBands: '5', legacyBandRaw: '0', nrOption: 'saNsa' }
  const B3: NetConfig = { ...B1_B3, lteBands: '4' }
  const searchView = (o: Partial<IpRefreshView> = {}) =>
    view({ method: 'networkSearch', phase: 'searching', tryN: 1, maxTries: 1, target: null, secondsInPhase: 42, secondsLeft: null, ...o })
  const ended = (r: Partial<IpRefreshResult> = {}, o: Partial<AppSnapshot> = {}): AppSnapshot =>
    snap({
      config: B1_B3,
      signal: { serving: 'lte', lte: { band: 3 } as never, nr: null },
      wan: { ipv4: null, range: '10.45', kind: 'private', ipv6: false },
      ipRefresh: view({
        active: false,
        phase: 'idle',
        method: null,
        last: {
          outcome: 'unchanged', tries: 1, target: null, from: '10.45', to: '10.45',
          method: 'networkSearch', refusal: null, bandsChanged: false, bandsBefore: null,
          ...r,
        },
      }),
      ...o,
    })

  beforeEach(() => {
    app.settings = settingsFixture()
    vi.mocked(api.ipRefreshStop).mockResolvedValue(undefined)
    vi.mocked(api.ipReattachStart).mockResolvedValue(undefined)
    vi.mocked(api.applyConfig).mockResolvedValue(undefined)
    vi.mocked(api.requestReboot).mockResolvedValue({ id: 'ticket-1', expiresInS: 30 } as never)
    vi.mocked(api.confirmReboot).mockResolvedValue(undefined)
  })
  afterEach(() => {
    cleanup()
    app.snapshot = null
    vi.clearAllMocks()
  })

  it('shows the search with its elapsed time, says it cannot be stopped midway, and stops after it', async () => {
    app.snapshot = snap({ ipRefresh: searchView() })
    render(IpRefreshBanner)
    const progress = screen.getByTestId('ipr-progress').textContent ?? ''
    expect(progress).toContain(t('ipRefresh.banner.phase.searching'))
    expect(progress).toContain(t('ipRefresh.banner.seconds', { n: '⁦42⁩' }))
    expect(screen.getByTestId('ipr-search-note').textContent).toBe(t('ipRefresh.banner.searchNote'))
    const stop = screen.getByTestId('ipr-stop') as HTMLButtonElement
    expect(stop.textContent).toBe(t('ipRefresh.banner.stopSearch'))
    await fireEvent.click(stop)
    expect(api.ipRefreshStop).toHaveBeenCalledTimes(1)
    expect(stop.textContent).toBe(t('ipRefresh.banner.stoppingSearch'))
  })

  it('words the online wait for a search, without the search note', () => {
    app.snapshot = snap({ ipRefresh: searchView({ phase: 'waitingOnline', secondsLeft: 100 }) })
    render(IpRefreshBanner)
    expect(screen.getByTestId('ipr-progress').textContent).toContain(t('ipRefresh.banner.phase.waitingOnlineSearch'))
    expect(screen.queryByTestId('ipr-search-note')).toBeNull()
  })

  it('words a stop after a search without mobile data', () => {
    app.snapshot = ended({ outcome: 'stopped' })
    render(IpRefreshBanner)
    expect(screen.getByTestId('ipr-outcome').textContent).toBe(
      t('ipRefresh.banner.ended', { outcome: t('ipRefresh.outcomeSearch.stopped') }),
    )
  })

  it('offers a band switch only after the outcomes it may help', () => {
    const cases: [IpRefreshResult['outcome'], IpRefreshResult['method'], boolean][] = [
      ['unchanged', 'networkSearch', true],
      ['notMatched', 'networkSearch', true],
      ['searchRefused', 'networkSearch', true],
      ['offRefused', 'dataSwitch', true],
      ['changed', 'networkSearch', false],
      ['matched', 'networkSearch', false],
      ['unchanged', 'dataSwitch', false],
    ]
    for (const [outcome, method, offered] of cases) {
      app.snapshot = ended({ outcome, method })
      render(IpRefreshBanner)
      expect(screen.queryByTestId('ipr-reattach') !== null, `${outcome} ${method}`).toBe(offered)
      cleanup()
    }
  })

  it('confirms the band switch with both steps before starting it', async () => {
    app.snapshot = ended()
    render(IpRefreshBanner)
    await fireEvent.click(screen.getByTestId('ipr-reattach'))
    expect(screen.getByRole('dialog')).toBeTruthy()
    expect(api.ipReattachStart).not.toHaveBeenCalled()
    expect(screen.getByTestId('ipr-reattach-step1').textContent).toBe('⁦B3⁩')
    expect(screen.getByTestId('ipr-reattach-final').textContent).toBe('⁦B1 + B3⁩')
    await fireEvent.click(screen.getByTestId('ipr-reattach-start'))
    expect(api.ipReattachStart).toHaveBeenCalledTimes(1)
    await flush()
    expect(screen.queryByRole('dialog')).toBeNull()
    // The result it came from no longer applies: its range and offers are gone.
    expect(screen.queryByTestId('ip-refresh-banner')).toBeNull()
  })

  it('hides the refresh result while a band switch runs', () => {
    const s = ended()
    app.snapshot = { ...s, ipReattach: { active: true, fromRange: '10.45', last: null } }
    render(IpRefreshBanner)
    expect(screen.queryByTestId('ip-refresh-banner')).toBeNull()
  })

  it('says automatic undo must be on, and does not start the switch without it', async () => {
    app.settings = settingsFixture({ automation: { primaryBand: null, relockPrimary: false, revertOnLoss: false } })
    app.snapshot = ended()
    render(IpRefreshBanner)
    await fireEvent.click(screen.getByTestId('ipr-reattach'))
    expect(screen.getByTestId('ipr-reattach-undo-off').textContent).toBe(t('ipRefresh.reattach.undoOff'))
    const start = screen.getByTestId('ipr-reattach-start') as HTMLButtonElement
    expect(start.disabled).toBe(true)
    await fireEvent.click(start)
    expect(api.ipReattachStart).not.toHaveBeenCalled()
  })

  it('keeps the confirm open with the refusal from the backend', async () => {
    vi.mocked(api.ipReattachStart).mockRejectedValue({
      kind: 'validation',
      message: 'too many band switches for a new IP in the last hour',
    })
    app.snapshot = ended()
    render(IpRefreshBanner)
    await fireEvent.click(screen.getByTestId('ipr-reattach'))
    await fireEvent.click(screen.getByTestId('ipr-reattach-start'))
    await flush()
    expect(screen.getByTestId('ipr-reattach-error').textContent).toBe(t('ipReattach.refuse.cooldown'))
    await fireEvent.click(screen.getByRole('button', { name: t('app.cancel') }))
    expect(screen.queryByRole('dialog')).toBeNull()
  })

  it('offers the two-step restart when the internet did not come back', async () => {
    app.snapshot = ended({ outcome: 'onlineTimeout', to: null })
    render(IpRefreshBanner)
    expect(screen.queryByTestId('ipr-reattach')).toBeNull()
    expect(screen.getByText(t('ipRefresh.banner.onlineTimeoutHelp'))).toBeTruthy()
    await fireEvent.click(screen.getByRole('button', { name: t('ipRefresh.banner.restart') }))
    await flush()
    expect(api.requestReboot).toHaveBeenCalledTimes(1)
    // The ticket only opens the confirm; nothing restarts without the second press.
    expect(screen.getByRole('dialog').textContent).toContain(t('reboot.title'))
    expect(api.confirmReboot).not.toHaveBeenCalled()
    await fireEvent.click(screen.getByRole('button', { name: t('reboot.confirm') }))
    expect(api.confirmReboot).toHaveBeenCalledWith('ticket-1')
  })

  it('restores the bands from before through a normal apply, only when they changed', async () => {
    app.snapshot = ended({ outcome: 'changed', to: '10.62', bandsChanged: true, bandsBefore: B3 })
    render(IpRefreshBanner)
    expect(screen.getByTestId('ipr-bands-changed').textContent).toContain('B3')
    await fireEvent.click(screen.getByTestId('ipr-restore'))
    expect(api.applyConfig).toHaveBeenCalledWith(B3)
    cleanup()
    // Already back on them: nothing to offer.
    app.snapshot = ended({ outcome: 'changed', to: '10.62', bandsChanged: true, bandsBefore: B3 }, { config: B3 })
    render(IpRefreshBanner)
    expect(screen.queryByTestId('ipr-restore')).toBeNull()
  })

  it('shows the code from the router when it refused', () => {
    app.snapshot = ended({ outcome: 'searchRefused', to: null, refusal: { kind: 'sessionLost', code: 100003 } })
    render(IpRefreshBanner)
    expect(screen.getByTestId('ipr-refusal-code').textContent).toBe(
      t('ipRefresh.banner.refusedCode', { code: '⁦100003⁩' }),
    )
    expect(screen.getByTestId('ipr-outcome-trouble')).toBeTruthy()
    cleanup()
    app.snapshot = ended({ outcome: 'offRefused', method: 'dataSwitch', to: null, refusal: { kind: 'timeout' } })
    render(IpRefreshBanner)
    expect(screen.queryByTestId('ipr-refusal-code')).toBeNull()
  })
})

describe('DataSwitchNotice', () => {
  beforeEach(() => {
    app.settings = settingsFixture()
    vi.mocked(api.dataSwitchOn).mockResolvedValue(undefined)
  })
  afterEach(() => {
    cleanup()
    app.snapshot = null
    vi.clearAllMocks()
  })

  it('offers the button when data is off with no record of the app doing it', async () => {
    app.snapshot = snap()
    const { rerender } = render(DataSwitchNotice)
    expect(screen.queryByTestId('data-switch-notice')).toBeNull()
    app.snapshot = snap({ dataSwitchOff: true })
    await rerender({})
    expect(screen.getByTestId('data-switch-notice').dataset.kind).toBe('off')
    await fireEvent.click(screen.getByRole('button', { name: t('ipRefresh.notice.turnOn') }))
    expect(api.dataSwitchOn).toHaveBeenCalledTimes(1)
  })

  it('shows a record it will not use, and a run that cannot log back in', async () => {
    app.snapshot = snap({ ipRefresh: view({ active: false, phase: 'idle', journalBlocked: true }) })
    render(DataSwitchNotice)
    expect(screen.getByTestId('data-switch-notice').dataset.kind).toBe('blocked')
    expect(screen.getByRole('button', { name: t('ipRefresh.notice.turnOn') })).toBeTruthy()
    cleanup()

    app.snapshot = snap({ session: { kind: 'badCredentials' }, ipRefresh: view({ phase: 'turningOn' }) })
    render(DataSwitchNotice)
    expect(screen.getByTestId('data-switch-notice').dataset.kind).toBe('stuck')
    // The run turns data on itself once logged in: no second button.
    expect(screen.queryByRole('button', { name: t('ipRefresh.notice.turnOn') })).toBeNull()
  })
})
