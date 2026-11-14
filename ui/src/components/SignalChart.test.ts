import { afterEach, describe, expect, it } from 'vitest'
import { cleanup, render } from '@testing-library/svelte'
import SignalChart from './SignalChart.svelte'

const series = (vs: (number | null)[], step = 1000) => vs.map((v, i) => ({ t: 1_000_000 + i * step, v }))

afterEach(cleanup)

describe('SignalChart', () => {
  it('breaks the line at null and labels the image', () => {
    const { container } = render(SignalChart, { samples: series([1, 2, null, 3, 4]), label: 'SINR' })
    expect(container.querySelectorAll('polyline').length).toBe(2)
    const svg = container.querySelector('svg')!
    expect(svg.getAttribute('role')).toBe('img')
    expect(svg.getAttribute('aria-label')).toContain('SINR')
  })

  it('breaks the line at a long time gap', () => {
    const samples = [...series([1, 2]), ...series([3, 4]).map((s) => ({ ...s, t: s.t + 60_000 }))]
    const { container } = render(SignalChart, { samples, label: 'SINR' })
    expect(container.querySelectorAll('polyline').length).toBe(2)
  })

  it('draws a lone point as a dot', () => {
    const { container } = render(SignalChart, { samples: series([null, 5, null]), label: 'SINR' })
    expect(container.querySelectorAll('polyline').length).toBe(0)
    expect(container.querySelectorAll('circle').length).toBe(1)
  })

  it('drops samples older than the window', () => {
    const samples = [{ t: 0, v: 1 }, { t: 1, v: 2 }, ...series([3, 4])]
    const { container } = render(SignalChart, { samples, label: 'SINR' })
    const lines = container.querySelectorAll('polyline')
    expect(lines.length).toBe(1)
    expect(lines[0].getAttribute('points')!.split(' ').length).toBe(2)
  })

  it('has round ticks, a zero line and the dashed reference', () => {
    const { container } = render(SignalChart, { samples: series([1, 2]), label: 'SINR', reference: 13 })
    const ticks = [...container.querySelectorAll('text')].map((n) => n.textContent)
    expect(ticks).toEqual(['-10', '0', '10', '20', '30'])
    expect(container.querySelectorAll('line.grid').length).toBe(4)
    expect(container.querySelectorAll('line.zero').length).toBe(1)
    expect(container.querySelectorAll('line.reference').length).toBe(1)
  })

  it('clamps values to the range', () => {
    const { container } = render(SignalChart, {
      samples: series([-50, 90]),
      label: 'SINR',
    })
    const ys = container.querySelector('polyline')!.getAttribute('points')!.split(' ').map((p) => Number(p.split(',')[1]))
    expect(ys[0]).toBeGreaterThan(ys[1])
    expect(Math.min(...ys)).toBeGreaterThanOrEqual(0)
    expect(Math.max(...ys)).toBeLessThanOrEqual(116)
  })
})
