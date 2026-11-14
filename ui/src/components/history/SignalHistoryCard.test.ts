import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/svelte'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))

import { app } from '../../lib/store.svelte'
import { t } from '../../lib/i18n'
import { history, resetHistory } from '../../lib/history.svelte'
import { pointFixture } from '../../lib/history.fixtures'
import { settingsFixture } from '../../test-fixtures'
import SignalHistoryCard from './SignalHistoryCard.svelte'

const MIN = 60_000
const from = new Date(2026, 10, 6).getTime()

function load(n: number) {
  history.signal = {
    bucket: 'min1',
    bucketMs: MIN,
    points: Array.from({ length: n }, (_, i) => pointFixture({ tMs: from + i * MIN })),
  }
  history.loadedRange = { fromMs: from, toMs: from + n * MIN, utcOffsetMin: 180 }
  history.events = {
    entries: [
      {
        kind: 'change',
        group: { startedMs: from + 2 * MIN, endedMs: from + 3 * MIN, from: [1, 3], to: [3], primary: null, outcome: { kind: 'kept' }, steps: [] },
      },
    ],
    nextBeforeMs: null,
  }
}

describe('SignalHistoryCard', () => {
  beforeEach(() => {
    resetHistory()
    app.settings = settingsFixture()
  })
  afterEach(() => cleanup())

  it('draws LTE solid and 5G dashed, and leaves 5G out of RSRQ without values', async () => {
    load(5)
    const { container } = render(SignalHistoryCard)
    expect(container.querySelector('polyline[data-series="lte"]')).not.toBeNull()
    expect(container.querySelector('polyline[data-series="nr"].dashed')).not.toBeNull()
    expect(screen.getByText(t('history.signal.legendNr'))).toBeTruthy()
    await fireEvent.click(screen.getByRole('radio', { name: t('history.signal.metric.rsrq') }))
    expect(container.querySelector('polyline[data-series="nr"]')).toBeNull()
    expect(screen.queryByText(t('history.signal.legendNr'))).toBeNull()
  })

  it('marks band changes and shows the allowed bands under the chart', () => {
    load(5)
    const { container } = render(SignalHistoryCard)
    expect(container.querySelector('g.marker.kept title')?.textContent).toBe(t('history.marker.kept', { bands: 'B3' }))
    // The legend names the kind in words, next to its own mark.
    expect(container.querySelector('[data-legend-marker="kept"]')?.textContent?.trim()).toBe(t('history.signal.legendMarker.kept'))
    expect(container.querySelector('[data-legend-marker="kept"] [data-glyph="kept"]')).not.toBeNull()
    expect(container.querySelector('.ribbon .span title')?.textContent).toBe('B3')
    expect(container.querySelectorAll('g.reference').length).toBe(3)
  })

  it('pages the table 24 rows at a time, newest first', async () => {
    load(30)
    render(SignalHistoryCard)
    const rows = () => screen.getAllByRole('row').slice(1)
    expect(rows().length).toBe(24)
    expect(screen.getByText(t('history.signal.page', { n: '1', total: '2' }))).toBeTruthy()
    await fireEvent.click(screen.getByRole('button', { name: t('history.signal.older') }))
    expect(rows().length).toBe(6)
  })

  it('says so when the range has no signal', () => {
    history.signal = { bucket: 'min1', bucketMs: MIN, points: [] }
    render(SignalHistoryCard)
    expect(screen.getByText(t('history.signal.empty'))).toBeTruthy()
    expect(screen.queryByRole('img')).toBeNull()
  })
})
