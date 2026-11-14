import { afterEach, beforeEach, describe, expect, it, onTestFinished, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))
vi.mock('../../lib/backend', () => ({ api: { historyEvents: vi.fn() } }))

import { api } from '../../lib/backend'
import { app } from '../../lib/store.svelte'
import { setLanguage, t } from '../../lib/i18n'
import { history, resetHistory } from '../../lib/history.svelte'
import { eventLog, resetEvents } from '../../lib/events.svelte'
import { settingsFixture } from '../../test-fixtures'
import type { EventItem } from '../../lib/bindings/EventItem'
import type { EventPage } from '../../lib/bindings/EventPage'
import EventLog from './EventLog.svelte'

// Noon today, so the events a few minutes earlier stay under "Today" even when the tests run just after midnight.
const now = new Date().setHours(12, 0, 0, 0)
const range = { fromMs: now - 86_400_000, toMs: now, utcOffsetMin: 180 }

const item = (o: Partial<EventItem> = {}): EventItem => ({
  atMs: now - 60_000,
  kind: 'app.started',
  category: 'app',
  severity: 'info',
  technical: false,
  key: 'events.kind.appStarted',
  params: {},
  detailKey: null,
  ...o,
})

const page = (): EventPage => ({
  entries: [
    {
      kind: 'change',
      group: {
        startedMs: now - 120_000,
        endedMs: now - 100_000,
        from: [1, 3],
        to: [3],
        primary: null,
        outcome: { kind: 'reverted', reason: 'safeApply.reason.connectionLost', fallback: false },
        steps: [
          item({ atMs: now - 120_000, kind: 'safe_apply.started', category: 'bandChange', key: 'events.kind.safeApplyStarted' }),
          item({ atMs: now - 119_000, kind: 'safe_apply.applied', category: 'bandChange', key: 'events.kind.safeApplyApplied', technical: true }),
          item({
            atMs: now - 100_000,
            kind: 'safe_apply.reverted',
            category: 'bandChange',
            severity: 'warning',
            key: 'events.kind.safeApplyReverted',
            params: { reason: { t: 'key', v: 'safeApply.reason.connectionLost' } },
          }),
        ],
      },
    },
    { kind: 'single', item: item({ atMs: now - 200_000, kind: 'session.lost', category: 'connection', severity: 'warning', key: 'events.kind.sessionLost', detailKey: 'events.why.sessionLost' }) },
  ],
  nextBeforeMs: now - 200_000,
})

async function openLog() {
  const view = render(EventLog)
  await screen.findAllByTestId('change')
  return view
}

describe('EventLog', () => {
  beforeEach(() => {
    resetHistory()
    resetEvents()
    app.settings = settingsFixture()
    history.loadedRange = range
    vi.mocked(api.historyEvents).mockResolvedValue(page())
  })
  afterEach(() => {
    cleanup()
    vi.clearAllMocks()
    setLanguage('en')
    app.historyFocus = null
  })

  it('loads the range once it is shown, and again for a new range', async () => {
    render(EventLog)
    await waitFor(() => expect(api.historyEvents).toHaveBeenCalledTimes(1))
    const next = { ...range, fromMs: range.fromMs - 86_400_000 }
    history.loadedRange = next
    await waitFor(() => expect(api.historyEvents).toHaveBeenCalledTimes(2))
    expect(vi.mocked(api.historyEvents).mock.calls[1][0].range).toEqual(next)
  })

  it('shows each event as a sentence with a severity word, under its day', async () => {
    await openLog()
    expect(screen.getByRole('heading', { level: 4, name: t('events.today') })).toBeTruthy()
    const change = screen.getByTestId('change')
    expect(within(change).getByRole('heading', { level: 5 }).textContent).toBe(
      'Tried ⁦B3⁩, then went back to ⁦B1 + B3⁩',
    )
    expect(change.textContent).toContain(t('events.whyLine', { reason: t('safeApply.reason.connectionLost') }))
    expect(change.textContent).toContain(t('events.severity.warning'))
    expect(screen.getByText(t('events.kind.sessionLost'))).toBeTruthy()
    expect(screen.getByText(t('events.why.sessionLost'))).toBeTruthy()
    expect(screen.getByRole('img', { name: t('events.category.connection') })).toBeTruthy()
  })

  it('asks again with the filters chosen', async () => {
    await openLog()
    expect(vi.mocked(api.historyEvents).mock.calls[0][0]).toMatchObject({ categories: [], problemsOnly: false, technical: false, beforeMs: null })
    await fireEvent.click(screen.getByRole('radio', { name: t('events.filter.other') }))
    await waitFor(() => expect(api.historyEvents).toHaveBeenCalledTimes(2))
    expect(vi.mocked(api.historyEvents).mock.calls[1][0].categories).toEqual(['device', 'app'])
    await fireEvent.click(screen.getByRole('checkbox', { name: t('events.problemsOnly') }))
    await waitFor(() => expect(api.historyEvents).toHaveBeenCalledTimes(3))
    expect(vi.mocked(api.historyEvents).mock.calls[2][0]).toMatchObject({ categories: ['device', 'app'], problemsOnly: true })
  })

  it('folds a change into a card whose steps open, without the technical ones until asked', async () => {
    await openLog()
    expect(screen.queryByTestId('steps')).toBeNull()
    const steps = screen.getByRole('button', { name: t('events.steps') })
    expect(steps.getAttribute('aria-expanded')).toBe('false')
    await fireEvent.click(steps)
    expect(steps.getAttribute('aria-expanded')).toBe('true')
    const list = screen.getByTestId('steps')
    expect(list.querySelectorAll('li')).toHaveLength(2)
    expect(list.textContent).not.toContain(t('events.kind.safeApplyApplied'))

    await fireEvent.click(screen.getByRole('checkbox', { name: t('events.technical') }))
    await waitFor(() => expect(vi.mocked(api.historyEvents).mock.calls.at(-1)?.[0].technical).toBe(true))
    await waitFor(() => expect(screen.getByTestId('steps').querySelectorAll('li')).toHaveLength(3))
  })

  it('loads older entries after the last one shown', async () => {
    await openLog()
    vi.mocked(api.historyEvents).mockResolvedValueOnce({
      entries: [{ kind: 'single', item: item({ atMs: now - 3 * 86_400_000, key: 'events.kind.deviceReboot', kind: 'device.reboot', category: 'device' }) }],
      nextBeforeMs: null,
    })
    await fireEvent.click(screen.getByRole('button', { name: t('events.older') }))
    await waitFor(() => expect(screen.getByText(t('events.kind.deviceReboot'))).toBeTruthy())
    expect(vi.mocked(api.historyEvents).mock.calls[1][0].beforeMs).toBe(now - 200_000)
    expect(screen.getByText(t('events.kind.sessionLost'))).toBeTruthy()
    expect(screen.queryByRole('button', { name: t('events.older') })).toBeNull()
  })

  it('opens on the newest band change when the banner sent the user here, once', async () => {
    const scroll = vi.fn()
    const before = Element.prototype.scrollIntoView
    Element.prototype.scrollIntoView = scroll
    onTestFinished(() => {
      Element.prototype.scrollIntoView = before
    })
    eventLog.filter = 'speedTest'
    app.historyFocus = 'latestChange'
    render(EventLog)
    await waitFor(() => expect(screen.getByTestId('steps')).toBeTruthy())
    expect(app.historyFocus).toBeNull()
    expect((screen.getByRole('radio', { name: t('events.filter.bandChange') }) as HTMLInputElement).checked).toBe(true)
    expect(vi.mocked(api.historyEvents).mock.calls[0][0].categories).toEqual(['bandChange'])
    await waitFor(() => expect(document.activeElement?.id).toBe(`ev-change-${now - 120_000}`))
    expect(scroll).toHaveBeenCalled()
    expect(api.historyEvents).toHaveBeenCalledTimes(1)
  })

  it('waits for the range the banner asked for before opening the newest change', async () => {
    const before = Element.prototype.scrollIntoView
    Element.prototype.scrollIntoView = vi.fn()
    onTestFinished(() => {
      Element.prototype.scrollIntoView = before
    })
    // The range kept from an earlier visit, while the 24h reload the banner started runs.
    history.loadedRange = { fromMs: now - 8 * 86_400_000, toMs: now - 86_400_000, utcOffsetMin: 180 }
    history.loading = true
    app.historyFocus = 'latestChange'
    render(EventLog)
    await Promise.resolve()
    expect(api.historyEvents).not.toHaveBeenCalled()
    expect(app.historyFocus).toBe('latestChange')
    // The new range arrives.
    history.loadedRange = range
    history.loading = false
    await waitFor(() => expect(screen.getByTestId('steps')).toBeTruthy())
    expect(app.historyFocus).toBeNull()
    expect(vi.mocked(api.historyEvents).mock.calls[0][0].range).toEqual(range)
    await waitFor(() => expect(document.activeElement?.id).toBe(`ev-change-${now - 120_000}`))
  })

  it('reads in Arabic with Arabic-Indic digits', async () => {
    setLanguage('ar')
    app.settings = settingsFixture({ language: 'ar', digits: 'arabicIndic' })
    await openLog()
    expect(within(screen.getByTestId('change')).getByRole('heading', { level: 5 }).textContent).toBe(
      'جُرّبت ⁦B٣⁩ ثم عادت ⁦B١ + B٣⁩',
    )
    expect(screen.getByText('تنبيه', { selector: '[data-testid="change"] .pill' })).toBeTruthy()
  })
})
