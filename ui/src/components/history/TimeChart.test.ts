import { afterEach, describe, expect, it } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/svelte'
import { execFileSync } from 'node:child_process'
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { sourceShape } from '../../lib/history.svelte'
import TimeChart, { timeTicks, TICK_PX } from './TimeChart.svelte'

const MIN = 60_000
const H = 60 * MIN
const D = 24 * H
const from = new Date(2026, 10, 6, 0, 0).getTime()

const base = {
  fromMs: from,
  toMs: from + 20 * MIN,
  min: -10,
  max: 30,
  ticks: [-10, 0, 10, 20, 30],
  unit: 'dB',
  label: 'SINR over time',
}
const v = (m: number, avg: number | null) => ({ t: from + m * MIN, min: avg === null ? null : avg - 1, avg, max: avg === null ? null : avg + 1 })

describe('TimeChart', () => {
  afterEach(() => cleanup())

  it('breaks a line at a gap longer than allowed, and at a missing value', () => {
    const { container } = render(TimeChart, {
      ...base,
      maxGapMs: 2 * MIN,
      series: [{ id: 'lte', label: 'LTE', style: 'solid', values: [v(0, 5), v(1, 6), v(2, 7), v(10, 8), v(11, 9), v(12, null), v(13, 4), v(14, 3)] }],
    })
    const lines = container.querySelectorAll('polyline[data-series="lte"]')
    expect(lines.length).toBe(3)
    expect(container.querySelectorAll('polygon.wash').length).toBe(3)
  })

  it('draws a second series dashed', () => {
    const { container } = render(TimeChart, {
      ...base,
      series: [
        { id: 'lte', label: 'LTE', style: 'solid', values: [v(0, 5), v(1, 6)] },
        { id: 'nr', label: '5G', style: 'dashed', values: [v(0, 15), v(1, 16)] },
      ],
    })
    expect(container.querySelector('polyline[data-series="nr"]')?.classList.contains('dashed')).toBe(true)
    expect(container.querySelector('polyline[data-series="lte"]')?.classList.contains('dashed')).toBe(false)
  })

  it('gives every marker a title, and leaves out markers outside the range', () => {
    const { container } = render(TimeChart, {
      ...base,
      series: [{ id: 'lte', label: 'LTE', style: 'solid', values: [v(0, 5), v(1, 6)] }],
      markers: [
        { t: from + 5 * MIN, kind: 'kept', label: 'Band change kept: B3' },
        { t: from + 7 * MIN, kind: 'reboot', label: 'Router restarted' },
        { t: from - D, kind: 'appStart', label: 'App started' },
      ],
    })
    const marks = container.querySelectorAll('g.marker')
    expect(marks.length).toBe(2)
    expect([...marks].map((m) => m.querySelector('title')?.textContent)).toEqual(['Band change kept: B3', 'Router restarted'])
  })

  it('gives each marker kind its own top mark, and names a marker in the readout of its stop', async () => {
    const { container } = render(TimeChart, {
      ...base,
      series: [{ id: 'lte', label: 'LTE', style: 'solid', values: [v(0, 5), v(5, 6), v(10, 7)] }],
      markers: [
        { t: from + 5 * MIN + 20_000, kind: 'kept', label: 'Band change kept: B3' },
        { t: from + 6 * MIN, kind: 'reverted', label: 'Band change undone: B1' },
        { t: from + 9 * MIN, kind: 'applied', label: 'Band change tried: B20' },
        { t: from + 10 * MIN, kind: 'reboot', label: 'Router restarted' },
        { t: from + 11 * MIN, kind: 'appStart', label: 'App started' },
      ],
    })
    const glyphs = [...container.querySelectorAll('g.marker [data-glyph]')]
    expect(glyphs.map((g) => g.getAttribute('data-glyph'))).toEqual(['kept', 'reverted', 'applied', 'reboot', 'appStart'])
    const drawn = glyphs.map((g) => g.firstElementChild?.tagName.toLowerCase())
    expect(new Set(drawn.slice(2)).size).toBe(3)
    expect(drawn.slice(0, 2)).toEqual(['path', 'path'])
    const chart = screen.getByRole('img', { name: 'SINR over time' })
    const readout = screen.getByRole('status')
    await fireEvent.keyDown(chart, { key: 'Home' })
    await fireEvent.keyDown(chart, { key: 'ArrowRight' })
    expect(readout.textContent).toContain('Band change kept: B3')
    expect(readout.textContent).toContain('Band change undone: B1')
    expect(readout.textContent).not.toContain('B20')
    await fireEvent.keyDown(chart, { key: 'End' })
    expect(readout.textContent).toContain('Band change tried: B20')
    expect(readout.textContent).toContain('Router restarted')
    expect(readout.textContent).toContain('App started')
  })

  it('draws each point in the shape of its source', () => {
    const sources = ['manual', 'scan', 'auto'] as const
    const { container } = render(TimeChart, {
      ...base,
      min: 0,
      max: 200,
      ticks: [0, 100, 200],
      unit: 'Mbps',
      points: sources.map((s, i) => ({ t: from + (i + 1) * MIN, v: 100, shape: sourceShape(s), label: s, id: s })),
    })
    const shape = (id: string) => container.querySelector(`g.pt[data-id="${id}"]`)
    expect(shape('manual')?.querySelector('circle')).not.toBeNull()
    expect(shape('scan')?.querySelector('rect')).not.toBeNull()
    expect(shape('auto')?.querySelector('polygon')).not.toBeNull()
    expect(shape('auto')?.getAttribute('data-shape')).toBe('triangle')
  })

  it('moves the readout with the arrow keys, Home and End', async () => {
    render(TimeChart, {
      ...base,
      series: [{ id: 'lte', label: 'LTE', style: 'solid', values: [v(0, 5), v(1, 6), v(2, 7)] }],
    })
    const chart = screen.getByRole('img', { name: 'SINR over time' })
    const readout = screen.getByRole('status')
    expect(readout.textContent?.trim()).toBe('')
    await fireEvent.keyDown(chart, { key: 'End' })
    expect(readout.textContent).toContain('LTE 7.0 dB')
    expect(readout.textContent).toContain('6.0 to 8.0')
    await fireEvent.keyDown(chart, { key: 'ArrowLeft' })
    expect(readout.textContent).toContain('LTE 6.0 dB')
    await fireEvent.keyDown(chart, { key: 'Home' })
    expect(readout.textContent).toContain('LTE 5.0 dB')
    await fireEvent.keyDown(chart, { key: 'ArrowLeft' })
    expect(readout.textContent).toContain('LTE 5.0 dB')
    await fireEvent.keyDown(chart, { key: 'ArrowRight' })
    expect(readout.textContent).toContain('LTE 6.0 dB')
    await fireEvent.keyDown(chart, { key: 'Escape' })
    expect(readout.textContent?.trim()).toBe('')
  })

  it('opens a point with Enter', async () => {
    const picked: string[] = []
    render(TimeChart, {
      ...base,
      points: [
        { t: from + MIN, v: 5, shape: 'circle', label: 'first', id: 'a' },
        { t: from + 2 * MIN, v: 6, shape: 'circle', label: 'second', id: 'b' },
      ],
      onselect: (id: string) => picked.push(id),
    })
    const chart = screen.getByRole('img')
    await fireEvent.keyDown(chart, { key: 'Home' })
    expect(screen.getByRole('status').textContent).toContain('first')
    await fireEvent.keyDown(chart, { key: 'Enter' })
    expect(picked).toEqual(['a'])
  })

  it('never writes a style attribute, which the strict CSP would block', () => {
    const { container } = render(TimeChart, {
      ...base,
      series: [{ id: 'lte', label: 'LTE', style: 'solid', values: [v(0, 5), v(1, 6)] }],
      ribbon: { label: 'Allowed bands', none: 'Nothing recorded', spans: [{ from, to: from + 10 * MIN, label: 'B1 + B3' }] },
    })
    expect(container.querySelector('[style]')).toBeNull()
    const markup = readFileSync(resolve(__dirname, 'TimeChart.svelte'), 'utf8').split('</script>').pop()!.split('<style>')[0]
    expect(markup).not.toMatch(/\sstyle(=|:)/)
  })

  it('hatches the ribbon where nothing was recorded, under the labelled spans', () => {
    const { container } = render(TimeChart, {
      ...base,
      series: [{ id: 'lte', label: 'LTE', style: 'solid', values: [v(0, 5), v(1, 6)] }],
      ribbon: { label: 'Allowed bands', none: 'Nothing recorded', spans: [{ from, to: from + 10 * MIN, label: 'B1 + B3' }] },
    })
    const none = container.querySelector('.ribbon rect.none')
    expect(none?.getAttribute('fill')).toMatch(/^url\(#.+\)$/)
    expect(container.querySelector('pattern')).not.toBeNull()
    expect(container.querySelector('.ribbon .span title')?.textContent).toBe('B1 + B3')
  })
})

describe('timeTicks', () => {
  it('leaves each label room and lands on whole local hours', () => {
    const ticks = timeTicks(from + 7 * MIN, from + D + 7 * MIN, 640)
    expect(ticks.length).toBeGreaterThan(1)
    expect(ticks.length).toBeLessThanOrEqual(Math.floor(640 / TICK_PX) + 1)
    for (const tk of ticks) {
      expect(new Date(tk.t).getMinutes()).toBe(0)
      expect(new Date(tk.t).getHours() % 3).toBe(0)
    }
    // Midnight shows the date instead of 00:00.
    expect(ticks.find((tk) => new Date(tk.t).getHours() === 0)?.label).not.toContain('00:00')
  })

  it('steps by days over a month, and by fewer when narrower', () => {
    const wide = timeTicks(from, from + 30 * D, 1200)
    const narrow = timeTicks(from, from + 30 * D, 300)
    for (const tk of wide) expect(new Date(tk.t).getHours()).toBe(0)
    expect(narrow.length).toBeLessThan(wide.length)
    expect(narrow.length).toBeLessThanOrEqual(Math.floor(300 / TICK_PX) + 1)
  })

  it('uses Arabic-Indic digits when asked', () => {
    const [first] = timeTicks(from + MIN, from + 6 * H, 640, 'ar', 'arabicIndic')
    expect(first.label).toMatch(/[٠-٩]/)
    expect(first.label).not.toMatch(/[0-9]/)
  })

  it('has nothing to show for an empty range', () => {
    expect(timeTicks(from, from, 640)).toEqual([])
  })
})

describe('logical CSS', () => {
  it('the chart files pass check-logical-css', () => {
    const root = resolve(__dirname, '../../../..')
    expect(() => execFileSync(process.execPath, [resolve(root, 'tools/check-logical-css.mjs')], { stdio: 'pipe' })).not.toThrow()
  })
})
