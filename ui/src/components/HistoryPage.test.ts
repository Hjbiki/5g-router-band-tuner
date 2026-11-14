import { afterEach, beforeEach, describe, expect, it, onTestFinished, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))
vi.mock('../lib/backend', () => ({
  api: {
    historyOverview: vi.fn(),
    historySignal: vi.fn(),
    historySpeedtests: vi.fn(),
    historyEvents: vi.fn(),
    scanPeriods: vi.fn(),
    applyConfig: vi.fn(),
  },
  onHistoryCleared: vi.fn(),
}))

import { api, onHistoryCleared } from '../lib/backend'
import { app, bandEdit } from '../lib/store.svelte'
import { setLanguage, t } from '../lib/i18n'
import { resetHistory, sections } from '../lib/history.svelte'
import { closeIpRefresh, ipRefreshUi } from '../lib/ipRefresh.svelte'
import { overviewFixture, pointFixture } from '../lib/history.fixtures'
import { resultFixture } from '../lib/speedtest.fixtures'
import { settingsFixture, snap } from '../test-fixtures'
import type { SpeedTestResult } from '../lib/bindings/SpeedTestResult'
import HistoryPage from './HistoryPage.svelte'

const D = 86_400_000

/** Opens a closed More details section by its summary, the way a click does. */
async function openSection(id: string) {
  const details = (await screen.findByTestId(`section-${id}`)) as HTMLDetailsElement
  details.open = true
  await fireEvent(details, new Event('toggle'))
  return details
}

function stubScroll() {
  const scroll = vi.fn()
  const before = Element.prototype.scrollIntoView
  Element.prototype.scrollIntoView = scroll
  onTestFinished(() => {
    Element.prototype.scrollIntoView = before
  })
  return scroll
}

const noSignal = () => vi.mocked(api.historySignal).mockResolvedValue({ bucket: 'min5', bucketMs: 300_000, points: [] })

describe('HistoryPage', () => {
  beforeEach(() => {
    try {
      localStorage.clear()
    } catch {
      // jsdom always has it
    }
    resetHistory()
    app.settings = settingsFixture()
    app.snapshot = snap()
    vi.mocked(onHistoryCleared).mockResolvedValue(() => {})
    vi.mocked(api.historyOverview).mockResolvedValue(overviewFixture())
    vi.mocked(api.historySignal).mockResolvedValue({
      bucket: 'min5',
      bucketMs: 300_000,
      points: [pointFixture({ tMs: 0, observedS: 300, serviceS: 300, nrShare: 0.5 })],
    })
    vi.mocked(api.historySpeedtests).mockResolvedValue([resultFixture({ downloadMbps: 120 })])
    vi.mocked(api.historyEvents).mockResolvedValue({ entries: [], nextBeforeMs: null })
    vi.mocked(api.scanPeriods).mockResolvedValue([])
  })
  afterEach(() => {
    cleanup()
    vi.clearAllMocks()
    setLanguage('en')
    app.historyFocus = null
  })

  it('shows skeletons, then the answer and the three cards', async () => {
    render(HistoryPage)
    expect(screen.getByTestId('history-skeleton')).toBeTruthy()
    await screen.findByTestId('history-answer')
    expect(screen.queryByTestId('history-skeleton')).toBeNull()
    const signal = within(screen.getByTestId('card-signal'))
    expect(signal.getByText('100%')).toBeTruthy()
    expect(signal.getByText(t('history.cards.on5g', { pct: '50%' }))).toBeTruthy()
    expect(signal.getByText(t('history.cards.lostNone'))).toBeTruthy()
    const speed = within(screen.getByTestId('card-speed'))
    expect(speed.getByText(t('speedtest.unit.speed', { n: '120' }))).toBeTruthy()
    expect(speed.getByText(t('history.cards.tests', { n: '1' }))).toBeTruthy()
    expect(within(screen.getByTestId('card-auto')).getByText(t('history.cards.autoOff'))).toBeTruthy()
    // The answer comes before the cards, and the details after them.
    const answer = screen.getByTestId('history-answer')
    expect(answer.compareDocumentPosition(screen.getByTestId('history-cards')) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    expect(answer.dataset.kind).toBe('oneSet')
  })

  it('says it is recording, and keeps the retention line in the footer', async () => {
    render(HistoryPage)
    expect((await screen.findByTestId('history-status')).textContent).toContain(t('history.status.recording'))
    const foot = screen.getByRole('contentinfo')
    expect(foot.textContent).toContain(t('history.footer.kept', { days: '90' }))
    expect(within(foot).getByRole('button', { name: t('history.range.pick') })).toBeTruthy()
  })

  it('says it is paused, with Connect, while not logged in', async () => {
    app.snapshot = snap({ session: { kind: 'reconnecting', reason: 'timeout', retryInS: 5 } })
    render(HistoryPage)
    expect((await screen.findByTestId('history-status')).textContent).toContain(t('history.status.paused'))
    await fireEvent.click(screen.getByRole('button', { name: t('history.status.connect') }))
    expect(app.tab).toBe('router')
    app.tab = 'history'
  })

  it('says the user logged out, with the way to the connection settings', async () => {
    app.snapshot = snap({ session: { kind: 'loggedOut' } })
    render(HistoryPage)
    expect((await screen.findByTestId('history-status')).textContent).toContain(t('history.status.loggedOut'))
    await fireEvent.click(screen.getByRole('button', { name: t('history.status.connect') }))
    expect(app.tab).toBe('router')
    app.tab = 'history'
  })

  it('says read only for a history from a newer version', async () => {
    vi.mocked(api.historyOverview).mockResolvedValue(overviewFixture({ readOnly: true }))
    render(HistoryPage)
    expect((await screen.findByTestId('history-status')).textContent).toContain(t('history.status.readOnly'))
    expect(screen.getByText(t('history.status.readOnlyHint'))).toBeTruthy()
  })

  it('opens the CSV export from the footer', async () => {
    render(HistoryPage)
    const open = await screen.findByRole('button', { name: t('history.export.open') })
    expect(open.closest('footer')).toBeTruthy()
    expect(screen.queryByTestId('history-export')).toBeNull()
    await fireEvent.click(open)
    expect(screen.getByTestId('history-export')).toBeTruthy()
    expect(open.getAttribute('aria-expanded')).toBe('true')
    await fireEvent.click(screen.getByRole('button', { name: t('app.cancel') }))
    expect(screen.queryByTestId('history-export')).toBeNull()
  })

  it('starts a new user on 7 days, and reloads with the bounds of the range picked', async () => {
    render(HistoryPage)
    await waitFor(() => expect(api.historySignal).toHaveBeenCalledTimes(1))
    const [first] = vi.mocked(api.historySignal).mock.calls[0]
    expect(first.toMs - first.fromMs).toBe(7 * D)
    const week = screen.getByRole('radio', { name: t('history.range.week', { n: '7' }) })
    expect(week.closest('label')?.classList.contains('on')).toBe(true)
    expect(screen.getAllByRole('radio')).toHaveLength(3)

    const day = screen.getByRole('radio', { name: t('history.range.day', { n: '24' }) })
    const before = Date.now()
    await fireEvent.click(day)
    await waitFor(() => expect(api.historySignal).toHaveBeenCalledTimes(2))
    const [range] = vi.mocked(api.historySignal).mock.calls[1]
    expect(range.toMs - range.fromMs).toBe(D)
    expect(range.toMs).toBeGreaterThanOrEqual(before)
    expect(day.closest('label')?.classList.contains('on')).toBe(true)
  })

  it('keeps a range picked before', async () => {
    localStorage.setItem('history.range', JSON.stringify({ kind: '30d' }))
    resetHistory()
    render(HistoryPage)
    await waitFor(() => expect(api.historySignal).toHaveBeenCalledTimes(1))
    const [range] = vi.mocked(api.historySignal).mock.calls[0]
    expect(range.toMs - range.fromMs).toBe(30 * D)
  })

  it('checks picked dates before loading them', async () => {
    render(HistoryPage)
    await waitFor(() => expect(api.historySignal).toHaveBeenCalledTimes(1))
    await fireEvent.click(await screen.findByRole('button', { name: t('history.range.pick') }))
    await waitFor(() => expect(api.historySignal).toHaveBeenCalledTimes(2))
    const [week] = vi.mocked(api.historySignal).mock.calls[1]
    expect(new Date(week.fromMs).getHours()).toBe(0)
    expect(Math.round((week.toMs + 1 - week.fromMs) / D)).toBe(7)
    // No preset is chosen while picked dates are shown, and the dates are named at the top.
    expect(screen.getAllByRole('radio').every((r) => !(r as HTMLInputElement).checked)).toBe(true)
    await waitFor(() => expect(screen.getByTestId('history-custom')).toBeTruthy())

    const from = screen.getByLabelText(t('history.range.from')) as HTMLInputElement
    const to = screen.getByLabelText(t('history.range.to')) as HTMLInputElement
    await fireEvent.input(from, { target: { value: to.value } })
    await fireEvent.input(to, { target: { value: '2020-01-01' } })
    await fireEvent.change(to)
    expect(screen.getByText(t('history.range.invalid'))).toBeTruthy()
    expect(to.getAttribute('aria-invalid')).toBe('true')
    await new Promise((r) => setTimeout(r, 250))
    expect(api.historySignal).toHaveBeenCalledTimes(2)
  })

  it('says when nothing was ever recorded', async () => {
    vi.mocked(api.historyOverview).mockResolvedValue(overviewFixture({ minutes: 0, speedtests: 0, events: 0 }))
    noSignal()
    vi.mocked(api.historySpeedtests).mockResolvedValue([])
    render(HistoryPage)
    expect(await screen.findByTestId('history-empty-fresh')).toBeTruthy()
    expect(screen.getByText(t('history.empty.freshTitle'))).toBeTruthy()
    expect(screen.queryByTestId('history-answer')).toBeNull()
    expect(screen.queryByRole('button', { name: t('history.export.open') })).toBeNull()
  })

  it('offers the last 30 days when the range is empty', async () => {
    noSignal()
    vi.mocked(api.historySpeedtests).mockResolvedValue([])
    render(HistoryPage)
    expect(await screen.findByTestId('history-empty-range')).toBeTruthy()
    await fireEvent.click(screen.getByRole('button', { name: t('history.empty.showLast', { n: '30' }) }))
    await waitFor(() => expect(api.historySignal).toHaveBeenCalledTimes(2))
    const [range] = vi.mocked(api.historySignal).mock.calls[1]
    expect(range.toMs - range.fromMs).toBe(30 * D)
    await waitFor(() => expect(screen.queryByRole('button', { name: t('history.empty.showLast', { n: '30' }) })).toBeNull())
  })

  it('says why when the database is unavailable', async () => {
    vi.mocked(api.historyOverview).mockResolvedValue(overviewFixture({ available: false, unavailableReason: 'disk full' }))
    render(HistoryPage)
    expect(await screen.findByTestId('history-empty-unavailable')).toBeTruthy()
    expect(screen.getByText(t('history.empty.reason', { reason: 'disk full' }))).toBeTruthy()
    expect(screen.queryByTestId('history-status')).toBeNull()
    expect(api.historySignal).not.toHaveBeenCalled()
  })

  it('shows a load failure inline with Retry', async () => {
    vi.mocked(api.historyOverview).mockRejectedValueOnce('boom')
    render(HistoryPage)
    const alert = await screen.findByRole('alert')
    expect(alert.textContent).toContain('boom')
    await fireEvent.click(screen.getByRole('button', { name: t('app.retry') }))
    await waitFor(() => expect(screen.queryByRole('alert')).toBeNull())
    expect(screen.getByTestId('history-answer')).toBeTruthy()
  })

  it('renders in Arabic, right to left, with Arabic-Indic digits', async () => {
    setLanguage('ar')
    app.settings = settingsFixture({ language: 'ar', digits: 'arabicIndic' })
    render(HistoryPage)
    await screen.findByTestId('history-answer')
    expect(document.documentElement.dir).toBe('rtl')
    expect(screen.getByRole('heading', { level: 1 }).textContent).toBe('السجل')
    expect(within(screen.getByTestId('card-signal')).getByText('١٠٠٪')).toBeTruthy()
    expect(screen.getByRole('radio', { name: 'آخر ٢٤ ساعة' })).toBeTruthy()
    expect(screen.getByRole('contentinfo').textContent).toContain('مدة حفظ السجلات بالأيام: ٩٠.')
    expect(screen.getByRole('heading', { name: 'مزيد من التفاصيل' })).toBeTruthy()
  })

  describe('More details', () => {
    it('starts every section closed, with nothing inside mounted, and ends with What happened', async () => {
      render(HistoryPage)
      const details = within(await screen.findByTestId('history-details'))
      const all = details.getAllByRole('group') as HTMLDetailsElement[]
      expect(all.map((d) => d.dataset.testid)).toEqual([
        'section-signal',
        'section-speeds',
        'section-hours',
        'section-bandSets',
        'section-events',
      ])
      expect(all.every((d) => !d.open)).toBe(true)
      expect(screen.queryByTestId('signal-history')).toBeNull()
      expect(screen.queryByTestId('event-log')).toBeNull()
      expect(api.historyEvents).toHaveBeenCalledTimes(1)
      expect(all.at(-1)?.textContent).toContain(t('history.details.events'))
    })

    it('mounts a section when opened, and remembers it on this PC', async () => {
      const first = render(HistoryPage)
      await openSection('events')
      await screen.findByTestId('event-log')
      // The log loads its own page of events when it opens.
      await waitFor(() => expect(api.historyEvents).toHaveBeenCalledTimes(2))
      expect(JSON.parse(localStorage.getItem('history.open')!)).toEqual(['events'])
      first.unmount()
      resetHistory()
      render(HistoryPage)
      expect(((await screen.findByTestId('section-events')) as HTMLDetailsElement).open).toBe(true)
      expect(sections.open).toEqual(['events'])
    })

    it('forgets a request for a section that an empty range has no room for', async () => {
      vi.mocked(api.historyOverview).mockResolvedValue(overviewFixture({ events: 0 }))
      noSignal()
      vi.mocked(api.historySpeedtests).mockResolvedValue([])
      app.historyFocus = 'bandSets'
      render(HistoryPage)
      expect(await screen.findByTestId('history-empty-range')).toBeTruthy()
      await waitFor(() => expect(app.historyFocus).toBeNull())
      expect(screen.queryByTestId('section-events')).toBeNull()
      expect(sections.open).toEqual([])
    })

    it('still shows What happened in a range without signal, and opens it on the newest change', async () => {
      noSignal()
      vi.mocked(api.historySpeedtests).mockResolvedValue([])
      app.historyFocus = 'latestChange'
      render(HistoryPage)
      expect(await screen.findByTestId('history-empty-range')).toBeTruthy()
      const log = (await screen.findByTestId('section-events')) as HTMLDetailsElement
      await waitFor(() => expect(log.open).toBe(true))
      expect(await screen.findByTestId('event-log')).toBeTruthy()
      await waitFor(() => expect(app.historyFocus).toBeNull())
      expect(sections.open).toEqual(['events'])
    })
  })

  describe('speed', () => {
    const serving = (lteBand: number, nrBand: number | null) => ({ ...resultFixture().serving, lteBand, nrBand })
    const tests = (): SpeedTestResult[] => [
      resultFixture({ atMs: 5_000, allowedBands: '5', downloadMbps: 90, source: 'manual', ipRange: '10.45' }),
      resultFixture({ atMs: 4_000, allowedBands: '5', downloadMbps: 110, ipRange: '10.45' }),
      resultFixture({ atMs: 3_000, allowedBands: '4', downloadMbps: 150, serving: serving(3, null), source: 'auto', ipRange: '10.46' }),
    ]

    beforeEach(() => {
      vi.mocked(api.historySpeedtests).mockResolvedValue(tests())
      // The second test ran inside an auto-scan and doesn't say so itself.
      vi.mocked(api.scanPeriods).mockResolvedValue([[3_500, 4_500]])
      bandEdit.draft = null
      bandEdit.dirty = false
    })

    it('lists the tests with what started each one, and filters by it', async () => {
      const view = render(HistoryPage)
      await openSection('speeds')
      const card = within(await screen.findByTestId('speed-history'))
      await waitFor(() => expect(card.getAllByTestId('source').map((c) => c.textContent)).toEqual(['You', 'Auto-scan', 'Automatic']))
      expect(view.container.querySelectorAll('g.pt')).toHaveLength(3)
      expect(view.container.querySelector('g.pt[data-shape="square"]')).toBeTruthy()

      const auto = card.getByRole('button', { name: t('speedtest.source.auto') })
      expect(auto.getAttribute('aria-pressed')).toBe('true')
      await fireEvent.click(auto)
      expect(auto.getAttribute('aria-pressed')).toBe('false')
      expect(card.getAllByTestId('source')).toHaveLength(2)
      expect(view.container.querySelectorAll('g.pt[data-shape="triangle"]')).toHaveLength(0)
    })

    it('opens the row of the point chosen with Enter', async () => {
      const list = Array.from({ length: 14 }, (_, i) => resultFixture({ atMs: 100_000 - i * 1000, downloadMbps: 50 + i }))
      vi.mocked(api.historySpeedtests).mockResolvedValue(list)
      const view = render(HistoryPage)
      await openSection('speeds')
      const chart = await screen.findByRole('img', { name: /each speed test/ })
      await fireEvent.keyDown(chart, { key: 'Home' })
      await fireEvent.keyDown(chart, { key: 'Enter' })
      // The oldest test is on the second page of 10.
      await waitFor(() => expect(view.container.querySelector('tr.opened')?.getAttribute('data-at')).toBe(String(list[13].atMs)))
      expect(document.activeElement?.getAttribute('data-at')).toBe(String(list[13].atMs))
    })

    it('says plainly when the range has signal but no tests, with no buttons', async () => {
      vi.mocked(api.historySpeedtests).mockResolvedValue([])
      render(HistoryPage)
      await openSection('speeds')
      const empty = await screen.findByTestId('speed-empty')
      expect(empty.textContent).toBe(t('history.speed.empty'))
      expect(within(screen.getByTestId('speed-history')).queryByRole('button')).toBeNull()
      expect(screen.getByTestId('history-answer').dataset.kind).toBe('noTests')
    })

    it('offers automatic tests from their card', async () => {
      render(HistoryPage)
      const card = within(await screen.findByTestId('card-auto'))
      expect(card.getByText(t('history.cards.autoRan', { n: '1' }))).toBeTruthy()
      await fireEvent.click(card.getByRole('button', { name: t('history.speed.turnOnAuto') }))
      expect(app.tab).toBe('settings')
      app.tab = 'history'
    })

    it('ranks band sets by 5G, then download, and sorts on a column', async () => {
      const view = render(HistoryPage)
      await openSection('bandSets')
      await screen.findByTestId('band-sets')
      const keys = () => [...view.container.querySelectorAll('[data-testid="band-sets"] tbody tr')].map((r) => r.getAttribute('data-key'))
      expect(keys()).toEqual(['set:5', 'set:4'])
      const set = view.container.querySelector('[data-testid="band-sets"] tbody tr')!
      expect(set.textContent).toContain(t('history.bands.of', { a: '2', b: '2' }))
      expect(set.textContent).toContain(t('history.bands.few'))
      const down = screen.getByRole('button', { name: new RegExp(t('history.bands.col.down')) })
      expect(down.closest('th')?.getAttribute('aria-sort')).toBe('none')
      await fireEvent.click(down)
      expect(down.closest('th')?.getAttribute('aria-sort')).toBe('descending')
      expect(keys()).toEqual(['set:4', 'set:5'])
      // One provider: no provider switch. "Nothing is scored" moved into a tip.
      expect(screen.queryByRole('radiogroup', { name: t('history.bands.provider') })).toBeNull()
      expect(screen.getByRole('button', { name: t('history.bands.noScoreLabel') })).toBeTruthy()
    })

    it('switches providers only when both measured', async () => {
      vi.mocked(api.historySpeedtests).mockResolvedValue([...tests(), resultFixture({ atMs: 2_000, provider: 'ookla', allowedBands: '1' })])
      const view = render(HistoryPage)
      await openSection('bandSets')
      const group = await screen.findByRole('radiogroup', { name: t('history.bands.provider') })
      await fireEvent.click(within(group).getByRole('radio', { name: t('speedtest.provider.ookla') }))
      const keys = [...view.container.querySelectorAll('[data-testid="band-sets"] tbody tr')].map((r) => r.getAttribute('data-key'))
      expect(keys).toEqual(['set:1'])
    })

    it('"Try these bands" fills the band editor and never applies', async () => {
      render(HistoryPage)
      await openSection('bandSets')
      await screen.findByTestId('band-sets')
      await fireEvent.click(screen.getByRole('button', { name: t('history.bands.tryLabel', { bands: '⁦B3⁩' }) }))
      expect(app.tab).toBe('bands')
      expect(bandEdit.dirty).toBe(true)
      expect(bandEdit.draft?.lteBands).toBe('4')
      expect(bandEdit.draft?.networkMode).toEqual(snap().config!.networkMode)
      expect(api.applyConfig).not.toHaveBeenCalled()
    })

    it('the answer card tries the fastest bands the same way', async () => {
      const at = (day: number) => new Date(2026, 10, day, 20).getTime()
      vi.mocked(api.historySpeedtests).mockResolvedValue([
        ...[150, 160, 170].map((d, i) => resultFixture({ atMs: at(2 + i), allowedBands: '4', downloadMbps: d })),
        ...[100, 100, 100].map((d, i) => resultFixture({ atMs: at(5 + i), allowedBands: '5', downloadMbps: d })),
      ])
      render(HistoryPage)
      const answer = await screen.findByTestId('history-answer')
      await waitFor(() => expect(answer.dataset.kind).toBe('fastest'))
      expect(answer.textContent).toContain(t('history.answer.fastest', { bands: '⁦B3⁩' }))
      await fireEvent.click(within(answer).getByRole('button', { name: t('history.bands.tryLabel', { bands: '⁦B3⁩' }) }))
      expect(app.tab).toBe('bands')
      expect(bandEdit.draft?.lteBands).toBe('4')
      expect(api.applyConfig).not.toHaveBeenCalled()
    })

    it('brings the band sets into view when another page asks, opens and keeps the section open', async () => {
      const scroll = stubScroll()
      app.historyFocus = 'bandSets'
      render(HistoryPage)
      await screen.findByTestId('band-sets')
      await waitFor(() => expect(scroll).toHaveBeenCalled())
      expect(app.historyFocus).toBeNull()
      expect(document.activeElement?.id).toBe('hs-sec-bandSets')
      expect(sections.open).toEqual(['bandSets'])
      expect(JSON.parse(localStorage.getItem('history.open')!)).toEqual(['bandSets'])
    })

    it('brings the answer into view when the Signal page asks', async () => {
      const scroll = stubScroll()
      app.historyFocus = 'answer'
      render(HistoryPage)
      await screen.findByTestId('history-answer')
      await waitFor(() => expect(document.activeElement?.id).toBe('history-answer'))
      expect(scroll).toHaveBeenCalled()
      expect(app.historyFocus).toBeNull()
      expect(sections.open).toEqual([])
    })

    it('shows speed by IP range, with too few tests not compared, and aims at a range without starting', async () => {
      stubScroll()
      const s = snap({ wan: { ipv4: '10.99.1.2', range: '10.99', kind: 'private', ipv6: false } })
      s.capabilities = { ...s.capabilities!, dataSwitch: true, ipMethods: ['dataSwitch'] }
      app.snapshot = s
      app.historyFocus = 'ipRanges'
      const view = render(HistoryPage)
      const table = await screen.findByTestId('ip-ranges')
      await waitFor(() => expect(document.activeElement?.id).toBe('hs-sec-ipRanges'))
      const rows = [...table.querySelectorAll('tbody tr')]
      expect(rows.map((r) => r.getAttribute('data-range'))).toEqual(['10.45', '10.46'])
      expect(rows[0].querySelector('bdi[dir="ltr"]')?.textContent).toBe('10.45.x.x')
      expect(rows[1].textContent).toContain(t('history.ip.notCompared'))
      // Too few tests: nothing to aim for.
      expect(rows[1].querySelector('button')).toBeNull()
      expect(view.container.querySelector('[data-testid="section-ipRanges"] .preview')).toBeNull()
      await fireEvent.click(within(rows[0] as HTMLElement).getByRole('button', { name: t('history.ip.aimLabel', { range: '⁦10.45.x.x⁩' }) }))
      // The Refresh IP dialog opens with the range filled in; nothing is sent until it's confirmed.
      expect(ipRefreshUi.open).toBe(true)
      expect(ipRefreshUi.prefill).toBe('10.45')
      closeIpRefresh()
    })

    it('offers "Aim for this range" only where Refresh IP can work, and not on the range in use', async () => {
      app.historyFocus = 'ipRanges'
      stubScroll()
      // A router without a data switch: no aim buttons at all.
      app.snapshot = snap()
      render(HistoryPage)
      const table = await screen.findByTestId('ip-ranges')
      expect(table.querySelectorAll('button')).toHaveLength(0)
      cleanup()
      // The range the router has now reads as such.
      const s = snap({ wan: { ipv4: '10.45.1.2', range: '10.45', kind: 'private', ipv6: false } })
      s.capabilities = { ...s.capabilities!, dataSwitch: true, ipMethods: ['dataSwitch'] }
      app.snapshot = s
      app.historyFocus = 'ipRanges'
      render(HistoryPage)
      const now = await screen.findByTestId('ip-range-now')
      expect(now.textContent).toBe(t('history.ip.current'))
      expect((await screen.findByTestId('ip-ranges')).querySelectorAll('button')).toHaveLength(0)
    })

    it('previews the faster range while its section is closed', async () => {
      vi.mocked(api.historySpeedtests).mockResolvedValue([
        ...[150, 160].map((d, i) => resultFixture({ atMs: i, downloadMbps: d, ipRange: '10.45' })),
        ...[100, 100].map((d, i) => resultFixture({ atMs: 10 + i, downloadMbps: d, ipRange: '10.46' })),
      ])
      render(HistoryPage)
      const section = await screen.findByTestId('section-ipRanges')
      await waitFor(() => expect(section.querySelector('.preview')?.textContent).toBe(t('history.ip.preview', { range: '⁦10.45.x.x⁩' })))
    })

    it('says so in one line when no test recorded its IP range', async () => {
      vi.mocked(api.historySpeedtests).mockResolvedValue(tests().map((r) => ({ ...r, ipRange: null })))
      app.historyFocus = 'ipRanges'
      render(HistoryPage)
      expect(await screen.findByTestId('ip-none')).toBeTruthy()
      expect(screen.queryByTestId('section-ipRanges')).toBeNull()
      await waitFor(() => expect(app.historyFocus).toBeNull())
    })
  })
})
