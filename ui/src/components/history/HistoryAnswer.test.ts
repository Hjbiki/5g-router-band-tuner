import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/svelte'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))
vi.mock('../../lib/backend', () => ({ api: { applyConfig: vi.fn() } }))

import { api } from '../../lib/backend'
import { app, bandEdit } from '../../lib/store.svelte'
import { setLanguage, t } from '../../lib/i18n'
import { history, resetHistory } from '../../lib/history.svelte'
import { resultFixture } from '../../lib/speedtest.fixtures'
import { settingsFixture, snap } from '../../test-fixtures'
import type { SpeedTestResult } from '../../lib/bindings/SpeedTestResult'
import HistoryAnswer from './HistoryAnswer.svelte'

const at = (day: number, hour = 20) => new Date(2026, 10, day, hour).getTime()
const tests = (hex: string, downs: number[], first = 2): SpeedTestResult[] =>
  downs.map((downloadMbps, i) => resultFixture({ atMs: at(first + i), allowedBands: hex, downloadMbps }))

describe('HistoryAnswer', () => {
  beforeEach(() => {
    resetHistory()
    app.settings = settingsFixture()
    app.snapshot = snap()
    bandEdit.draft = null
    bandEdit.dirty = false
  })
  afterEach(() => {
    cleanup()
    vi.clearAllMocks()
    setLanguage('en')
    app.historyFocus = null
    app.tab = 'history'
  })

  it('offers an auto-scan when there is nothing to compare', async () => {
    history.speeds = []
    render(HistoryAnswer)
    expect(screen.getByText(t('history.answer.noTests'))).toBeTruthy()
    expect(screen.queryByRole('button', { name: t('history.bands.try') })).toBeNull()
    await fireEvent.click(screen.getByRole('button', { name: t('history.answer.scan') }))
    expect(app.tab).toBe('scan')
  })

  it('says how many tests are still needed', () => {
    history.speeds = [...tests('4', [100, 110, 120]), ...tests('5', [60], 9)]
    render(HistoryAnswer)
    expect(screen.getByText(t('history.answer.fewTestsHint', { min: '3', n: '4' }))).toBeTruthy()
  })

  it('names the fastest set, how much faster, when it did best, and what it rests on', async () => {
    history.speeds = [...tests('4', [150, 160, 170]), ...tests('4', [155], 20).map((r) => ({ ...r, atMs: at(20, 9) })), ...tests('5', [100, 100, 100], 9)]
    render(HistoryAnswer)
    const card = screen.getByTestId('history-answer')
    expect(card.dataset.kind).toBe('fastest')
    expect(card.textContent).toContain(t('history.answer.fastest', { bands: '⁦B3⁩' }))
    expect(card.textContent).toContain(
      t('history.answer.lead', { speed: t('speedtest.unit.speed', { n: '158' }), pct: '57%', other: '⁦B1 + B3⁩' }),
    )
    expect(card.textContent).toContain(t('history.answer.dayPart.evening'))
    expect(card.textContent).toContain(t('history.answer.basis', { provider: 'Cloudflare', n: '7' }))
    // No auto-scan to run: the answer is clear. The table is one click away.
    expect(screen.queryByRole('button', { name: t('history.answer.scan') })).toBeNull()
    await fireEvent.click(screen.getByRole('button', { name: t('history.answer.seeTable') }))
    expect(app.historyFocus).toBe('bandSets')
  })

  it('calls two sets too close, and never applies anything', () => {
    history.speeds = [...tests('4', [115, 118, 119]), ...tests('5', [100, 100, 100], 9)]
    render(HistoryAnswer)
    expect(screen.getByText(t('history.answer.tooClose', { a: '⁦B3⁩', b: '⁦B1 + B3⁩' }))).toBeTruthy()
    expect(screen.getByText(t('history.answer.tooCloseHint', { pct: '20%' }))).toBeTruthy()
    expect(screen.queryByRole('button', { name: t('history.bands.try') })).toBeNull()
    expect(api.applyConfig).not.toHaveBeenCalled()
  })

  it('offers a scan, not "try these bands", when every test ran on one band set', () => {
    history.speeds = tests('4', [100, 120])
    render(HistoryAnswer)
    expect(screen.getByTestId('history-answer').dataset.kind).toBe('oneSet')
    expect(screen.queryByRole('button', { name: t('history.bands.tryLabel', { bands: '⁦B3⁩' }) })).toBeNull()
    expect(screen.getByRole('button', { name: t('history.answer.scan') })).toBeTruthy()
    expect(screen.getByText(t('history.answer.oneSetHint'))).toBeTruthy()
  })

  it("can't try bands without a band config to start from", () => {
    app.snapshot = snap({ config: null })
    history.speeds = [...tests('4', [150, 160, 170]), ...tests('5', [100, 100, 100], 9)]
    render(HistoryAnswer)
    expect((screen.getByRole('button', { name: t('history.bands.tryLabel', { bands: '⁦B3⁩' }) }) as HTMLButtonElement).disabled).toBe(true)
  })

  it('reads in Arabic with Arabic-Indic digits', () => {
    setLanguage('ar')
    app.settings = settingsFixture({ language: 'ar', digits: 'arabicIndic' })
    history.speeds = tests('4', [100, 120])
    render(HistoryAnswer)
    // Band names stay as written, like on the Bands page; the counts use the chosen digits.
    expect(screen.getByText('جرت كل الاختبارات في هذه الفترة على ⁦B3⁩، فلا شيء لمقارنتها به.')).toBeTruthy()
    expect(screen.getByText('من اختبارات السرعة عبر Cloudflare في هذه الفترة. عدد الاختبارات: ٢.')).toBeTruthy()
  })
})
