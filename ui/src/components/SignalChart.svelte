<script lang="ts">
  import { formatNumber, type DigitStyle } from '../lib/format'

  // One series over a fixed time window ending at the newest sample. Values outside the
  // y range are clamped to its edge; nulls and gaps longer than `maxGapMs` break the line.
  let {
    samples,
    label,
    min = -10,
    max = 30,
    ticks = [-10, 0, 10, 20, 30],
    reference = null,
    windowMs = 300_000,
    maxGapMs = 10_000,
    digits = 'western',
  }: {
    samples: { t: number; v: number | null }[]
    label: string
    min?: number
    max?: number
    ticks?: number[]
    reference?: number | null
    windowMs?: number
    maxGapMs?: number
    digits?: DigitStyle
  } = $props()

  const MIN_H = 116
  const TOP = 6
  const BOTTOM = 6
  const LEFT = 30
  const RIGHT = 4

  let width = $state(0)
  let height = $state(0)
  let wrap: HTMLDivElement | undefined = $state()
  // Follow the card width so text and strokes keep their size (no viewBox stretching).
  $effect(() => {
    if (!wrap || typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(([entry]) => {
      width = entry.contentRect.width
      height = entry.contentRect.height
    })
    ro.observe(wrap)
    return () => ro.disconnect()
  })
  // jsdom (and the first frame) report 0; draw at a sensible width until measured.
  const W = $derived(width > 0 ? width : 320)
  const H = $derived(Math.max(MIN_H, height))
  const plotW = $derived(W - LEFT - RIGHT)
  const span = $derived(max - min || 1)
  const y = (v: number) => TOP + (1 - (Math.min(max, Math.max(min, v)) - min) / span) * (H - TOP - BOTTOM)

  const runs = $derived.by(() => {
    const end = samples.at(-1)?.t ?? 0
    const start = end - windowMs
    const out: { x: number; y: number }[][] = []
    let cur: { x: number; y: number }[] = []
    let prevT: number | null = null
    for (const s of samples) {
      if (s.t < start) continue
      const gap = prevT !== null && s.t - prevT > maxGapMs
      prevT = s.t
      if (s.v === null || !Number.isFinite(s.v) || gap) {
        if (cur.length) out.push(cur)
        cur = []
        if (s.v === null || !Number.isFinite(s.v)) continue
      }
      cur.push({ x: LEFT + ((s.t - start) / windowMs) * plotW, y: y(s.v) })
    }
    if (cur.length) out.push(cur)
    return out
  })

  const pts = (run: { x: number; y: number }[]) => run.map((p) => `${p.x.toFixed(1)},${p.y.toFixed(1)}`).join(' ')
  // The wash under the line, down to the chart's floor.
  const area = (run: { x: number; y: number }[]) => {
    const floor = (H - BOTTOM).toFixed(1)
    return `${run[0].x.toFixed(1)},${floor} ${pts(run)} ${run[run.length - 1].x.toFixed(1)},${floor}`
  }
  const newest = $derived(runs.at(-1)?.at(-1) ?? null)
  const gradient = $props.id()
</script>

<div class="wrap" bind:this={wrap}>
  <svg width={W} height={H} viewBox="0 0 {W} {H}" role="img" aria-label={label}>
    <defs>
      <linearGradient id={gradient} x1="0" y1="0" x2="0" y2="1">
        <stop class="wash-top" offset="0%" />
        <stop class="wash-end" offset="100%" />
      </linearGradient>
    </defs>
    <g aria-hidden="true">
      {#each ticks as tick (tick)}
        <line class={tick === 0 ? 'zero' : 'grid'} x1={LEFT} x2={W - RIGHT} y1={y(tick)} y2={y(tick)} />
        <text class="tick" x={LEFT - 6} y={y(tick)} dy="0.32em">{formatNumber(tick, { digits })}</text>
      {/each}
      {#if reference !== null}
        <line class="reference" x1={LEFT} x2={W - RIGHT} y1={y(reference)} y2={y(reference)} />
      {/if}
    </g>
    {#each runs as run, i (i)}
      {#if run.length > 1}
        <polygon points={area(run)} fill="url(#{gradient})" />
        <polyline class="series" points={pts(run)} />
      {:else}
        <circle class="dot" cx={run[0].x} cy={run[0].y} r="2" />
      {/if}
    {/each}
    {#if newest && (runs.at(-1)?.length ?? 0) > 1}<circle class="now" cx={newest.x} cy={newest.y} r="3.5" />{/if}
  </svg>
</div>

<style>
  /* Fills what its card has left; the drawing is out of flow, so it can't hold the card open. */
  .wrap {
    position: relative;
    flex: 1;
    min-block-size: 116px;
    inline-size: 100%;
    /* Time always runs oldest to newest, left to right, also in RTL. */
    direction: ltr;
  }
  svg {
    position: absolute;
    inset-block-start: 0;
    inset-inline-start: 0;
    display: block;
    max-inline-size: 100%;
  }
  .grid {
    stroke: var(--chart-grid);
    stroke-width: 1;
  }
  .zero {
    stroke: var(--chart-zero);
    stroke-width: 1.5;
  }
  .reference {
    stroke: var(--q-good);
    stroke-width: 1;
    stroke-dasharray: 4 3;
  }
  .tick {
    fill: var(--muted);
    font-size: var(--fs-small);
    font-variant-numeric: tabular-nums;
    text-anchor: end;
  }
  .series {
    fill: none;
    stroke: var(--accent);
    stroke-width: 2;
    stroke-linejoin: round;
    stroke-linecap: round;
  }
  .dot {
    fill: var(--accent);
  }
  .now {
    fill: var(--accent);
    stroke: var(--surface);
    stroke-width: 2;
  }
  .wash-top {
    stop-color: var(--accent);
    stop-opacity: 0.2;
  }
  .wash-end {
    stop-color: var(--accent);
    stop-opacity: 0;
  }
</style>
