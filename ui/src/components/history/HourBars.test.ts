import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, render, screen } from '@testing-library/svelte'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))

import { app } from '../../lib/store.svelte'
import { setLanguage, t } from '../../lib/i18n'
import { history, resetHistory } from '../../lib/history.svelte'
import { resultFixture } from '../../lib/speedtest.fixtures'
import { settingsFixture } from '../../test-fixtures'
import HourBars from './HourBars.svelte'

const at = (hour: number, day = 2) => new Date(2026, 10, day, hour, 10).getTime()

describe('HourBars', () => {
  beforeEach(() => {
    resetHistory()
    app.settings = settingsFixture()
  })
  afterEach(() => {
    cleanup()
    setLanguage('en')
  })

  it('asks for more tests below six valid ones', () => {
    history.speeds = [
      ...Array.from({ length: 5 }, (_, i) => resultFixture({ atMs: at(8, i + 1) })),
      resultFixture({ atMs: at(9), invalidReason: 'bandChange' }),
    ]
    render(HourBars)
    expect(screen.getByTestId('hours-few').textContent).toBe(t('history.empty.fewTests', { min: '6', n: '5' }))
    expect(screen.queryByTestId('hour-bars')).toBeNull()
  })

  it('draws one bar per hour with tests, hatched for a single test, and a table with the same numbers', () => {
    history.speeds = [
      ...[100, 50, 80, 120, 90].map((d, i) => resultFixture({ atMs: at(8, i + 1), downloadMbps: d })),
      resultFixture({ atMs: at(21), downloadMbps: 30 }),
    ]
    const view = render(HourBars)
    const bars = view.container.querySelectorAll('g.hour')
    expect(bars).toHaveLength(2)
    expect(bars[0].getAttribute('data-hour')).toBe('8')
    expect(bars[0].querySelectorAll('line.whisker').length).toBeGreaterThan(0)
    expect(bars[1].querySelector('rect.hatched')).toBeTruthy()
    expect(bars[1].querySelector('line.whisker')).toBeNull()
    expect(bars[0].querySelector('title')?.textContent).toBe(t('history.hours.bar', { hour: '08:00', value: '90.0', n: '5' }))
    expect(screen.getByText(t('history.hours.caption', { n: '6' }))).toBeTruthy()
    // Labels every 3 hours, midnight first.
    const labels = [...view.container.querySelectorAll('text.xtick')].map((x) => x.textContent)
    expect(labels).toEqual(['00:00', '03:00', '06:00', '09:00', '12:00', '15:00', '18:00', '21:00'])
    const rows = view.container.querySelectorAll('tbody tr')
    expect(rows).toHaveLength(2)
    expect(rows[1].textContent).toContain('—')
  })

  it('labels the hours in Arabic-Indic digits', () => {
    setLanguage('ar')
    app.settings = settingsFixture({ language: 'ar', digits: 'arabicIndic' })
    history.speeds = Array.from({ length: 6 }, (_, i) => resultFixture({ atMs: at(3, i + 1), downloadMbps: 40 + i }))
    const view = render(HourBars)
    const labels = [...view.container.querySelectorAll('text.xtick')].map((x) => x.textContent)
    expect(labels[1]).toBe('٠٣:٠٠')
    expect(screen.getByText('وسيط سرعة التنزيل حسب الساعة التي أُجري فيها كل اختبار، من اختبارات السرعة في هذه الفترة. عدد الاختبارات: ٦')).toBeTruthy()
  })
})
