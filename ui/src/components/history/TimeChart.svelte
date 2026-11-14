<script module lang="ts">
  import { formatNumber, type DigitStyle } from '../../lib/format'

  /** One bucket of a series: its average, and the lowest and highest value in it when known. */
  export type ChartValue = { t: number; min?: number | null; avg: number | null; max?: number | null }
  export type ChartSeries = { id: string; label: string; values: ChartValue[]; style: 'solid' | 'dashed' }
  /** A single measurement, such as one speed test; the shape says what started it. */
  export type ChartPoint = { t: number; v: number; shape: 'circle' | 'square' | 'triangle'; label: string; id: string }
  /** A moment worth a vertical line, such as a band change. `label` is its tooltip. */
  export type ChartMarker = { t: number; kind: string; label: string }
  /** A horizontal guide line, such as where a signal turns good. */
  export type ChartReference = { v: number; label: string }
  /** A strip under the plot: what applied when (the allowed bands). Uncovered time is hatched. */
  export type ChartRibbon = { label: string; none: string; spans: { from: number; to: number; label: string }[] }
  export type TimeTick = { t: number; label: string }

  const MIN = 60_000
  const HOUR = 60 * MIN
  const DAY = 24 * HOUR
  const STEPS = [15 * MIN, 30 * MIN, HOUR, 2 * HOUR, 3 * HOUR, 6 * HOUR, 12 * HOUR, DAY, 2 * DAY, 7 * DAY, 14 * DAY, 28 * DAY]
  /** Room each time label needs along the axis. */
  export const TICK_PX = 90

  const numbering = (digits: DigitStyle) => (digits === 'arabicIndic' ? 'arab' : 'latn')

  export function formatTime(t: number, lang: string, digits: DigitStyle): string {
    return new Intl.DateTimeFormat(lang, { hour: '2-digit', minute: '2-digit', hourCycle: 'h23', numberingSystem: numbering(digits) }).format(t)
  }
  export function formatDay(t: number, lang: string, digits: DigitStyle): string {
    return new Intl.DateTimeFormat(lang, { day: 'numeric', month: 'short', numberingSystem: numbering(digits) }).format(t)
  }

  /**
   * Time labels for an axis `px` wide: the smallest step that leaves each label `TICK_PX`, on
   * local clock times (whole hours, midnights). Below a day, a label at midnight shows the date.
   */
  export function timeTicks(fromMs: number, toMs: number, px: number, lang = 'en', digits: DigitStyle = 'western'): TimeTick[] {
    const span = toMs - fromMs
    if (!(span > 0) || !(px > 0)) return []
    const most = Math.max(2, Math.floor(px / TICK_PX))
    const step = STEPS.find((s) => span / s <= most) ?? STEPS[STEPS.length - 1]
    const first = new Date(fromMs)
    const midnight = new Date(first.getFullYear(), first.getMonth(), first.getDate())
    const out: TimeTick[] = []
    if (step < DAY) {
      let t = midnight.getTime()
      while (t < fromMs) t += step
      for (; t <= toMs; t += step) {
        const d = new Date(t)
        const atMidnight = d.getHours() === 0 && d.getMinutes() === 0
        out.push({ t, label: atMidnight ? formatDay(t, lang, digits) : formatTime(t, lang, digits) })
      }
    } else {
      const days = Math.round(step / DAY)
      const d = midnight
      while (d.getTime() < fromMs) d.setDate(d.getDate() + 1)
      for (; d.getTime() <= toMs; d.setDate(d.getDate() + days)) out.push({ t: d.getTime(), label: formatDay(d.getTime(), lang, digits) })
    }
    return out
  }

  export function fmtValue(v: number | null | undefined, digits: DigitStyle, fraction: number): string {
    return formatNumber(v ?? null, { digits, fraction })
  }
</script>

<script lang="ts">
  import { t, currentLanguage } from '../../lib/i18n'
  import MarkerGlyph from './MarkerGlyph.svelte'

  // A time chart drawn as inline SVG with CSS-token colours. Each series is an average line with
  // a wash from its lowest to its highest value; points are single measurements. Time always runs
  // left to right, also in RTL. The arrow keys, Home and End move a readout from bucket to bucket.
  let {
    fromMs,
    toMs,
    series = [],
    points = [],
    markers = [],
    references = [],
    ribbon = null,
    min,
    max,
    ticks,
    unit,
    label,
    digits = 'western',
    fraction = 1,
    maxGapMs = Infinity,
    onselect,
  }: {
    fromMs: number
    toMs: number
    series?: ChartSeries[]
    points?: ChartPoint[]
    markers?: ChartMarker[]
    references?: ChartReference[]
    ribbon?: ChartRibbon | null
    min: number
    max: number
    ticks: number[]
    unit: string
    label: string
    digits?: DigitStyle
    fraction?: number
    maxGapMs?: number
    /** Enter on a point. */
    onselect?: (id: string) => void
  } = $props()

  const H = 200
  const TOP = 8
  const LEFT = 40
  const END = 10
  const AXIS = 22
  const RIB = 14

  let width = $state(0)
  let wrap: HTMLDivElement | undefined = $state()
  let readoutEl: HTMLDivElement | undefined = $state()
  $effect(() => {
    if (!wrap || typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(([entry]) => (width = entry.contentRect.width))
    ro.observe(wrap)
    return () => ro.disconnect()
  })
  // jsdom and the first frame report 0; draw at a sensible width until measured.
  const W = $derived(width > 0 ? width : 640)
  const plotW = $derived(Math.max(1, W - LEFT - END))
  const plotBottom = $derived(H - AXIS - (ribbon ? RIB + 6 : 0))
  const span = $derived(toMs - fromMs || 1)
  const yspan = $derived(max - min || 1)

  const x = (tm: number) => LEFT + ((tm - fromMs) / span) * plotW
  const y = (v: number) => TOP + (1 - (Math.min(max, Math.max(min, v)) - min) / yspan) * (plotBottom - TOP)
  const f1 = (n: number) => n.toFixed(1)

  type Pt = { x: number; yAvg: number; yMin: number; yMax: number }
  // Each series split into runs: a null or a gap longer than `maxGapMs` breaks the line.
  const drawn = $derived(
    series.map((s) => {
      const runs: Pt[][] = []
      let cur: Pt[] = []
      let prev: number | null = null
      for (const v of s.values) {
        if (v.t < fromMs || v.t > toMs) continue
        const gap = prev !== null && v.t - prev > maxGapMs
        if (v.avg === null || !Number.isFinite(v.avg) || gap) {
          if (cur.length) runs.push(cur)
          cur = []
        }
        if (v.avg === null || !Number.isFinite(v.avg)) continue
        prev = v.t
        cur.push({ x: x(v.t), yAvg: y(v.avg), yMin: y(v.min ?? v.avg), yMax: y(v.max ?? v.avg) })
      }
      if (cur.length) runs.push(cur)
      return { s, runs }
    }),
  )
  const line = (run: Pt[]) => run.map((p) => `${f1(p.x)},${f1(p.yAvg)}`).join(' ')
  const wash = (run: Pt[]) =>
    [...run.map((p) => `${f1(p.x)},${f1(p.yMax)}`), ...[...run].reverse().map((p) => `${f1(p.x)},${f1(p.yMin)}`)].join(' ')

  const lang = $derived(currentLanguage())
  const xTicks = $derived(timeTicks(fromMs, toMs, plotW, lang, digits))
  const shownMarkers = $derived(markers.filter((m) => m.t >= fromMs && m.t <= toMs))
  const shownPoints = $derived(points.filter((p) => p.t >= fromMs && p.t <= toMs).sort((a, b) => a.t - b.t))
  const pointsMode = $derived(series.length === 0)

  // What the readout steps through: every bucket with a value, or every point.
  const timeline = $derived.by((): number[] => {
    if (pointsMode) return shownPoints.map((p) => p.t)
    const set = new Set<number>()
    for (const s of series) for (const v of s.values) if (v.avg !== null && v.t >= fromMs && v.t <= toMs) set.add(v.t)
    return [...set].sort((a, b) => a - b)
  })
  let active: number | null = $state(null)
  const activeT = $derived(active !== null && active < timeline.length ? timeline[active] : null)

  // The smallest step between two readout stops: a marker this close to the active stop, and
  // nearer to it than to any other, is named in the readout, so the arrow keys reach it too.
  const step = $derived.by(() => {
    let s = Infinity
    for (let i = 1; i < timeline.length; i++) s = Math.min(s, timeline[i] - timeline[i - 1])
    return Number.isFinite(s) ? s : span
  })
  const activeMarkers = $derived(
    active === null || activeT === null
      ? []
      : shownMarkers.filter((m) => Math.abs(m.t - activeT) <= step && nearest(m.t) === active).map((m) => m.label),
  )

  function whenText(tm: number): string {
    return `${formatDay(tm, lang, digits)} ${formatTime(tm, lang, digits)}`
  }
  const readout = $derived.by((): string => {
    if (activeT === null || active === null) return ''
    const withMarkers = (values: string) => [values, ...activeMarkers].filter(Boolean).join(' · ')
    if (pointsMode) return t('history.chart.readout', { time: whenText(activeT), values: withMarkers(shownPoints[active]?.label ?? '') })
    const parts: string[] = []
    for (const s of series) {
      const v = s.values.find((p) => p.t === activeT)
      if (!v || v.avg === null) continue
      let text = `${s.label} ${fmtValue(v.avg, digits, fraction)} ${unit}`
      if (v.min != null && v.max != null && v.min !== v.max) {
        text += ` (${t('history.chart.span', { min: fmtValue(v.min, digits, fraction), max: fmtValue(v.max, digits, fraction) })})`
      }
      parts.push(text)
    }
    return t('history.chart.readout', { time: whenText(activeT), values: withMarkers(parts.join(' · ')) })
  })

  // The readout's place is set through CSSOM, which the strict CSP allows; never a style attribute.
  $effect(() => {
    if (!readoutEl || activeT === null) return
    readoutEl.style.setProperty('--x', `${f1(x(activeT))}px`)
  })

  function nearest(tm: number): number | null {
    if (!timeline.length) return null
    let lo = 0
    let hi = timeline.length - 1
    while (lo < hi) {
      const mid = (lo + hi) >> 1
      if (timeline[mid] < tm) lo = mid + 1
      else hi = mid
    }
    return lo > 0 && tm - timeline[lo - 1] < timeline[lo] - tm ? lo - 1 : lo
  }

  function onPointer(e: PointerEvent) {
    const svg = e.currentTarget as SVGSVGElement
    const box = svg.getBoundingClientRect()
    const scale = box.width > 0 ? W / box.width : 1
    const px = (e.clientX - box.left) * scale
    active = nearest(fromMs + ((px - LEFT) / plotW) * span)
  }

  function onKey(e: KeyboardEvent) {
    const last = timeline.length - 1
    if (last < 0) return
    const cur = active ?? last
    let next: number | null = null
    switch (e.key) {
      case 'ArrowLeft':
        next = Math.max(0, active === null ? last : cur - 1)
        break
      case 'ArrowRight':
        next = Math.min(last, active === null ? last : cur + 1)
        break
      case 'Home':
        next = 0
        break
      case 'End':
        next = last
        break
      case 'Escape':
        active = null
        return
      case 'Enter':
        if (pointsMode && active !== null && shownPoints[active]) {
          e.preventDefault()
          onselect?.(shownPoints[active].id)
        }
        return
      default:
        return
    }
    e.preventDefault()
    active = next
  }

  const hatch = $props.id()
  const triangle = (px: number, py: number) => `${f1(px)},${f1(py - 5)} ${f1(px + 5)},${f1(py + 4)} ${f1(px - 5)},${f1(py + 4)}`
</script>

<div class="tc" bind:this={wrap}>
  <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
  <svg
    width={W}
    height={H}
    viewBox="0 0 {W} {H}"
    role="img"
    aria-label={label}
    tabindex="0"
    onkeydown={onKey}
    onpointermove={onPointer}
    onpointerleave={() => (active = null)}
    onblur={() => (active = null)}
  >
    {#if ribbon}
      <defs>
        <pattern id={hatch} width="6" height="6" patternUnits="userSpaceOnUse" patternTransform="rotate(45)">
          <line class="hatch-line" x1="0" y1="0" x2="0" y2="6" />
        </pattern>
      </defs>
    {/if}
    <g aria-hidden="true">
      {#each ticks as tick (tick)}
        <line class={tick === 0 ? 'zero' : 'grid'} x1={LEFT} x2={W - END} y1={y(tick)} y2={y(tick)} />
        <text class="ytick" x={LEFT - 6} y={y(tick)} dy="0.32em">{formatNumber(tick, { digits })}</text>
      {/each}
      {#each xTicks as tk (tk.t)}
        <line class="grid" x1={x(tk.t)} x2={x(tk.t)} y1={TOP} y2={plotBottom} />
        <text class="xtick" x={x(tk.t)} y={H - 6}>{tk.label}</text>
      {/each}
    </g>
    {#each references as r (r.v)}
      <g class="reference"><title>{r.label}</title><line x1={LEFT} x2={W - END} y1={y(r.v)} y2={y(r.v)} /></g>
    {/each}
    {#each drawn as d, si (d.s.id)}
      {#each d.runs as run, i (i)}
        {#if run.length > 1}
          <polygon class="wash s{si}" points={wash(run)} />
          <polyline class="series s{si}" class:dashed={d.s.style === 'dashed'} data-series={d.s.id} points={line(run)} />
        {:else}
          <circle class="dot s{si}" data-series={d.s.id} cx={run[0].x} cy={run[0].yAvg} r="2.5" />
        {/if}
      {/each}
    {/each}
    {#each shownPoints as p, i (p.id)}
      <g class="pt" class:active={pointsMode && active === i} data-shape={p.shape} data-id={p.id}>
        <title>{p.label}</title>
        {#if p.shape === 'square'}
          <rect x={x(p.t) - 4} y={y(p.v) - 4} width="8" height="8" />
        {:else if p.shape === 'triangle'}
          <polygon points={triangle(x(p.t), y(p.v))} />
        {:else}
          <circle cx={x(p.t)} cy={y(p.v)} r="4.5" />
        {/if}
      </g>
    {/each}
    {#each shownMarkers as m, i (i)}
      <g class="marker {m.kind}" data-kind={m.kind}>
        <title>{m.label}</title>
        <line class="mark" x1={x(m.t)} x2={x(m.t)} y1={TOP + 5} y2={plotBottom} />
        <line class="hit" x1={x(m.t)} x2={x(m.t)} y1={TOP} y2={plotBottom} />
        <MarkerGlyph kind={m.kind} cx={x(m.t)} cy={TOP} />
      </g>
    {/each}
    {#if ribbon}
      <g class="ribbon">
        <title>{ribbon.label}</title>
        <rect class="none" fill="url(#{hatch})" x={LEFT} y={plotBottom + 6} width={plotW} height={RIB}><title>{ribbon.none}</title></rect>
        {#each ribbon.spans as sp, i (i)}
          {@const x0 = Math.max(LEFT, x(sp.from))}
          {@const x1 = Math.min(W - END, x(sp.to))}
          {#if x1 > x0}
            <g class="span {i % 2 ? 'b' : 'a'}">
              <title>{sp.label}</title>
              <rect x={x0} y={plotBottom + 6} width={x1 - x0} height={RIB} />
              {#if x1 - x0 > sp.label.length * 7 + 8}
                <text x={(x0 + x1) / 2} y={plotBottom + 6 + RIB / 2} dy="0.34em">{sp.label}</text>
              {/if}
            </g>
          {/if}
        {/each}
      </g>
    {/if}
    {#if activeT !== null}
      <line class="crosshair" x1={x(activeT)} x2={x(activeT)} y1={TOP} y2={plotBottom} />
    {/if}
  </svg>
  <div class="readout" class:shown={activeT !== null} role="status" aria-live="polite" bind:this={readoutEl}>
    <!-- The box sits on the left-to-right time axis; its words follow their own language. -->
    <span dir="auto">{readout}</span>
  </div>
</div>

<style>
  /* Time always runs oldest to newest, left to right, also in RTL. */
  .tc {
    position: relative;
    inline-size: 100%;
    direction: ltr;
  }
  svg {
    display: block;
    max-inline-size: 100%;
    border-radius: var(--radius-control);
    touch-action: pan-y;
  }
  svg:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  .grid {
    stroke: var(--chart-grid);
    stroke-width: 1;
  }
  .zero {
    stroke: var(--chart-zero);
    stroke-width: 1.5;
  }
  .ytick,
  .xtick {
    fill: var(--muted);
    font-size: var(--fs-small);
    font-variant-numeric: tabular-nums;
  }
  .ytick {
    text-anchor: end;
  }
  .xtick {
    text-anchor: middle;
  }
  .reference line {
    stroke: var(--q-good);
    stroke-width: 1;
    stroke-dasharray: 2 4;
    opacity: 0.8;
  }
  .series {
    fill: none;
    stroke: var(--accent);
    stroke-width: 2;
    stroke-linejoin: round;
    stroke-linecap: round;
  }
  .series.dashed {
    stroke-dasharray: 6 4;
  }
  .wash {
    fill: var(--accent);
    opacity: 0.14;
  }
  .dot {
    fill: var(--accent);
  }
  .s1 {
    stroke: var(--chart-2);
  }
  .wash.s1,
  .dot.s1 {
    fill: var(--chart-2);
    stroke: none;
  }
  .pt {
    fill: var(--accent);
    stroke: var(--surface);
    stroke-width: 1.5;
  }
  .pt.active {
    stroke: var(--text);
    stroke-width: 2;
  }
  .marker .mark {
    stroke: var(--muted);
    stroke-width: 1.5;
    stroke-dasharray: 3 3;
  }
  .marker.kept .mark {
    stroke: var(--q-excellent);
  }
  .marker.reverted .mark {
    stroke: var(--q-poor);
  }
  .marker .hit {
    stroke: transparent;
    stroke-width: 10;
  }
  .hatch-line {
    stroke: var(--control);
    stroke-width: 2;
  }
  .span.a rect {
    fill: var(--active);
  }
  .span.b rect {
    fill: var(--raised);
  }
  .span rect {
    stroke: var(--border);
    stroke-width: 1;
  }
  .span text {
    fill: var(--text);
    font-size: 11px;
    text-anchor: middle;
  }
  .crosshair {
    stroke: var(--text);
    stroke-width: 1;
    opacity: 0.5;
    pointer-events: none;
  }
  .readout {
    position: absolute;
    inset-block-start: 4px;
    inset-inline-start: clamp(0px, calc(var(--x, 0px) - 120px), calc(100% - 240px));
    max-inline-size: 240px;
    padding-block: 4px;
    padding-inline: 8px;
    border: 1px solid var(--border);
    border-radius: var(--radius-control);
    background: var(--surface);
    box-shadow: var(--shadow-card);
    font-size: var(--fs-small);
    font-variant-numeric: tabular-nums;
    pointer-events: none;
    opacity: 0;
  }
  .readout.shown {
    opacity: 1;
  }
</style>
