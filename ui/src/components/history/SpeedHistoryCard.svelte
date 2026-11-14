<script lang="ts">
  // The History page's speed tests: each speed test in the range as a point, shaped by what started
  // it, with the band changes marked. Enter on a point shows it in the table below. The More details
  // section around it gives the heading.
  import { app } from '../../lib/store.svelte'
  import { t, currentLanguage } from '../../lib/i18n'
  import { formatNumber } from '../../lib/format'
  import { mbps, ms } from '../../lib/speedtest.svelte'
  import type { SpeedTestResult } from '../../lib/bindings/SpeedTestResult'
  import type { SpeedTestSource } from '../../lib/bindings/SpeedTestSource'
  import {
    SPEED_METRICS,
    SPEED_SOURCES,
    history,
    niceAxis,
    rangeBounds,
    signalMarkers,
    sourceShape,
    speedSource,
    speedValue,
    validSpeed,
    type SpeedMetric,
  } from '../../lib/history.svelte'
  import SpeedHistoryTable from '../SpeedHistoryTable.svelte'
  import MarkerLegend from './MarkerLegend.svelte'
  import TimeChart, { formatDay, formatTime, type ChartPoint } from './TimeChart.svelte'

  const digits = $derived(app.settings?.digits ?? 'western')
  const lang = $derived(currentLanguage())
  let metric = $state<SpeedMetric>('down')

  const sourceOf = (r: SpeedTestResult): SpeedTestSource => speedSource(r, history.scanPeriods)
  const all = $derived([...history.speeds].sort((a, b) => b.atMs - a.atMs))
  const present = $derived(SPEED_SOURCES.filter((s) => all.some((r) => sourceOf(r) === s)))
  // Sources switched off; a source that isn't in the range is never in the way.
  let hidden: SpeedTestSource[] = $state([])
  const toggle = (s: SpeedTestSource) => (hidden = hidden.includes(s) ? hidden.filter((x) => x !== s) : [...hidden, s])
  const rows = $derived(all.filter((r) => !hidden.includes(sourceOf(r))))

  const bounds = $derived(history.loadedRange ?? rangeBounds(history.range, Date.now()))
  const isPing = $derived(metric === 'ping')
  const unit = $derived(t(isPing ? 'speedtest.unit.ms' : 'speedtest.unit.mbps'))
  const fmt = (v: number) => (isPing ? ms(v, digits) : mbps(v, digits))
  const points = $derived.by((): ChartPoint[] =>
    rows.flatMap((r) => {
      const v = validSpeed(r) ? speedValue(r, metric) : null
      if (v === null) return []
      const source = sourceOf(r)
      const value = t(isPing ? 'speedtest.unit.millis' : 'speedtest.unit.speed', { n: fmt(v) })
      return [{ t: r.atMs, v, shape: sourceShape(source), id: String(r.atMs), label: t('history.speed.point', { source: t(`speedtest.source.${source}`), value }) }]
    }),
  )
  const axis = $derived(niceAxis(Math.max(0, ...points.map((p) => p.v)) * 1.05))
  const fromMs = $derived(Math.min(bounds.fromMs, ...points.map((p) => p.t)))
  const toMs = $derived(Math.max(bounds.toMs, ...points.map((p) => p.t)))
  const markers = $derived(signalMarkers(history.events))
  const chartLabel = $derived(
    t('history.speed.chartLabel', {
      metric: t(`history.speed.metric.${metric}`),
      from: `${formatDay(fromMs, lang, digits)} ${formatTime(fromMs, lang, digits)}`,
      to: `${formatDay(toMs, lang, digits)} ${formatTime(toMs, lang, digits)}`,
    }),
  )

  let open: { atMs: number } | null = $state(null)
</script>

<div class="wrap" data-testid="speed-history">
  {#if all.length === 0}
    <p class="muted" data-testid="speed-empty">{t('history.speed.empty')}</p>
  {:else}
    <div class="controls">
      <div class="seg" role="radiogroup" aria-label={t('history.speed.metricLabel')}>
        {#each SPEED_METRICS as m (m)}
          <label class:on={metric === m}>
            <input type="radio" name="hs-speed-metric" checked={metric === m} onchange={() => (metric = m)} />
            {t(`history.speed.metric.${m}`)}
          </label>
        {/each}
      </div>
      {#if present.length > 1}
        <div class="chips" role="group" aria-label={t('history.speed.sourceLabel')}>
          {#each present as s (s)}
            <button type="button" class="chip" aria-pressed={!hidden.includes(s)} onclick={() => toggle(s)}>
              <svg width="12" height="12" aria-hidden="true" class="shape">
                {#if sourceShape(s) === 'square'}<rect x="2" y="2" width="8" height="8" />
                {:else if sourceShape(s) === 'triangle'}<polygon points="6,1 11,10 1,10" />
                {:else}<circle cx="6" cy="6" r="4.5" />{/if}
              </svg>
              {t(`speedtest.source.${s}`)}
            </button>
          {/each}
        </div>
      {/if}
    </div>

    <ul class="legend" aria-label={t('history.speed.legend')}>
      {#each present as s (s)}
        <li>
          <svg width="12" height="12" aria-hidden="true" class="shape">
            {#if sourceShape(s) === 'square'}<rect x="2" y="2" width="8" height="8" />
            {:else if sourceShape(s) === 'triangle'}<polygon points="6,1 11,10 1,10" />
            {:else}<circle cx="6" cy="6" r="4.5" />{/if}
          </svg>
          {t(s === 'manual' ? 'history.speed.legendManual' : s === 'scan' ? 'history.speed.legendScan' : 'history.speed.legendAuto')}
        </li>
      {/each}
      <MarkerLegend {markers} />
    </ul>

    {#if rows.length === 0}
      <p class="muted">{t('history.speed.noneShown')}</p>
    {:else}
      <TimeChart
        {fromMs}
        {toMs}
        {points}
        {markers}
        min={0}
        max={axis.max}
        ticks={axis.ticks}
        {unit}
        label={chartLabel}
        {digits}
        fraction={isPing ? 0 : 1}
        onselect={(id) => (open = { atMs: Number(id) })}
      />
      <p class="muted small">{t('history.speed.keys')}</p>
      <SpeedHistoryTable {rows} {sourceOf} {open} caption={t('history.speed.tableCaption')} />
    {/if}
  {/if}
</div>

<style>
  .wrap {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .controls {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px 16px;
  }
  .seg {
    display: inline-flex;
    flex-wrap: wrap;
    gap: 2px;
    padding: 2px;
    border-radius: var(--radius-control);
    background: var(--active);
  }
  .seg label {
    position: relative;
    display: inline-flex;
    align-items: center;
    block-size: 28px;
    padding-inline: 12px;
    border-radius: 5px;
    font-size: var(--fs-small);
    color: var(--muted);
    cursor: pointer;
  }
  .seg label:hover {
    color: var(--text);
  }
  .seg label.on {
    background: var(--surface);
    color: var(--text);
    font-weight: 500;
    box-shadow: var(--shadow-thumb), 0 0 0 1px var(--control);
  }
  .seg label:focus-within {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }
  .seg input {
    position: absolute;
    opacity: 0;
    pointer-events: none;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    block-size: 28px;
    padding-inline: 10px;
    border-radius: 999px;
    font-size: var(--fs-small);
    color: var(--muted);
  }
  .chip[aria-pressed='true'] {
    color: var(--text);
    border-color: var(--accent);
    box-shadow: 0 0 0 1px var(--accent);
  }
  .shape {
    fill: var(--accent);
    flex: none;
  }
  .chip[aria-pressed='false'] .shape {
    fill: none;
    stroke: var(--muted);
    stroke-width: 1.5;
  }
  .legend {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 16px;
    margin: 0;
    padding: 0;
    list-style: none;
    font-size: var(--fs-small);
    color: var(--muted);
  }
  .legend li {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .muted {
    margin: 0;
    color: var(--muted);
  }
  .small {
    font-size: var(--fs-small);
  }
</style>
