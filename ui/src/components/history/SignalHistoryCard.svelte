<script lang="ts">
  // The History page's signal over time: one metric over the range, LTE and 5G, with the band
  // changes, restarts and the allowed bands under it. A table below gives the same numbers in words.
  // The More details section around it gives the heading.
  import { app } from '../../lib/store.svelte'
  import { t, currentLanguage } from '../../lib/i18n'
  import { THRESHOLDS, formatNumber } from '../../lib/format'
  import {
    METRIC_AXIS,
    SIGNAL_METRICS,
    bandRibbon,
    bandsText,
    history,
    metricValues,
    rangeBounds,
    signalMarkers,
    type SignalMetric,
  } from '../../lib/history.svelte'
  import { supportedBands } from '../../lib/bands'
  import InfoTip from '../ui/InfoTip.svelte'
  import MarkerLegend from './MarkerLegend.svelte'
  import TimeChart, { fmtValue, formatDay, formatTime, type ChartSeries } from './TimeChart.svelte'

  const ROWS_PER_PAGE = 24
  const DAY = 86_400_000

  const digits = $derived(app.settings?.digits ?? 'western')
  const lang = $derived(currentLanguage())
  let metric: SignalMetric = $state('sinr')

  const series = $derived(history.signal)
  const points = $derived(series?.points ?? [])
  const bucketMs = $derived(series?.bucketMs ?? 60_000)
  // The range the data was loaded for; a range change shows the old chart until the new one lands.
  const bounds = $derived(history.loadedRange ?? rangeBounds(history.range, Date.now()))
  const fromMs = $derived(points.length ? Math.min(bounds.fromMs, points[0].tMs) : bounds.fromMs)
  const toMs = $derived(points.length ? Math.max(bounds.toMs, points[points.length - 1].tMs + bucketMs) : bounds.toMs)

  const axis = $derived(METRIC_AXIS[metric])
  const values = $derived(metricValues(points, metric))
  const metricName = $derived(t(`history.signal.metric.${metric}`))
  const chartSeries = $derived.by((): ChartSeries[] => {
    const out: ChartSeries[] = [{ id: 'lte', label: t('history.signal.lte'), values: values.lte, style: 'solid' }]
    if (values.nr) out.push({ id: 'nr', label: t('history.signal.nr'), values: values.nr, style: 'dashed' })
    return out
  })
  const QUALITIES = ['excellent', 'good', 'fair'] as const
  const references = $derived(
    THRESHOLDS[metric].map((v, i) => ({
      v,
      label: t('history.signal.threshold', {
        quality: t(`quality.${QUALITIES[i]}`),
        value: `${formatNumber(v, { digits })} ${axis.unit}`,
      }),
    })),
  )
  const markers = $derived(signalMarkers(history.events))
  const ribbon = $derived({
    label: t('history.signal.ribbon'),
    none: t('history.signal.ribbonNone'),
    spans: bandRibbon(points, bucketMs),
  })
  const chartLabel = $derived(
    t('history.signal.chartLabel', {
      metric: metricName,
      from: `${formatDay(fromMs, lang, digits)} ${formatTime(fromMs, lang, digits)}`,
      to: `${formatDay(toMs, lang, digits)} ${formatTime(toMs, lang, digits)}`,
    }),
  )

  // The table: newest first, a page at a time.
  let page = $state(0)
  const rows = $derived([...points].reverse())
  const pages = $derived(Math.max(1, Math.ceil(rows.length / ROWS_PER_PAGE)))
  $effect(() => {
    if (page >= pages) page = pages - 1
  })
  const shown = $derived(rows.slice(page * ROWS_PER_PAGE, (page + 1) * ROWS_PER_PAGE))
  const when = (tm: number) =>
    bucketMs >= DAY ? formatDay(tm, lang, digits) : `${formatDay(tm, lang, digits)} ${formatTime(tm, lang, digits)}`
  const cell = (v: { min: number | null; avg: number | null; max: number | null }) => {
    if (v.avg === null) return '—'
    const avg = fmtValue(v.avg, digits, 1)
    return v.min !== null && v.max !== null && v.min !== v.max
      ? `${avg} (${t('history.chart.span', { min: fmtValue(v.min, digits, 1), max: fmtValue(v.max, digits, 1) })})`
      : avg
  }
  const pct = (part: number, whole: number) =>
    whole > 0 ? t('history.cards.percent', { n: formatNumber(Math.round((part / whole) * 100), { digits }) }) : '—'
  const num = (n: number) => formatNumber(n, { digits })
</script>

<div class="wrap" data-testid="signal-history">
  <div class="head">
    <div class="seg" role="radiogroup" aria-label={t('history.signal.metricLabel')}>
      {#each SIGNAL_METRICS as m (m)}
        <label class:on={metric === m}>
          <input type="radio" name="hs-metric" checked={metric === m} onchange={() => (metric = m)} />
          {t(`history.signal.metric.${m}`)}
        </label>
      {/each}
    </div>
    <InfoTip label={t('history.signal.nrCachedLabel')} text={t('history.signal.nrCached')} />
    {#if points.length}<InfoTip label={t('history.signal.keysLabel')} text={t('history.signal.keys')} />{/if}
  </div>

  {#if points.length === 0}
    <p class="muted">{t('history.signal.empty')}</p>
  {:else}
    <ul class="legend" aria-label={t('history.signal.legend')}>
      <li>
        <svg width="24" height="10" aria-hidden="true"><line class="lte" x1="1" x2="23" y1="5" y2="5" /></svg>
        {t('history.signal.legendLte')}
      </li>
      {#if values.nr}
        <li>
          <svg width="24" height="10" aria-hidden="true"><line class="nr" x1="1" x2="23" y1="5" y2="5" /></svg>
          {t('history.signal.legendNr')}
        </li>
      {/if}
      <li>
        <svg width="24" height="10" aria-hidden="true"><rect class="wash" x="1" y="1" width="22" height="8" /></svg>
        {t('history.signal.legendSpread')}
      </li>
      <MarkerLegend {markers} />
    </ul>
    <TimeChart
      {fromMs}
      {toMs}
      series={chartSeries}
      {markers}
      {references}
      {ribbon}
      min={axis.min}
      max={axis.max}
      ticks={axis.ticks}
      unit={axis.unit}
      label={chartLabel}
      {digits}
      maxGapMs={2 * bucketMs}
    />

    <details>
      <summary>{t('history.table.show')}</summary>
      <div class="table-wrap">
        <table>
          <caption class="sr-only">{t('history.signal.tableCaption', { metric: metricName })}</caption>
          <thead>
            <tr>
              <th scope="col">{t('history.signal.col.time')}</th>
              <th scope="col">{t('history.signal.col.lte', { metric: metricName, unit: axis.unit })}</th>
              <th scope="col">{t('history.signal.col.nr', { metric: metricName, unit: axis.unit })}</th>
              <th scope="col">{t('history.signal.col.service')}</th>
              <th scope="col">{t('history.signal.col.bands')}</th>
            </tr>
          </thead>
          <tbody>
            {#each shown as p (p.tMs)}
              {@const m = metricValues([p], metric)}
              <tr>
                <td>{when(p.tMs)}</td>
                <td class="num">{cell(m.lte[0])}</td>
                <td class="num">{m.nr ? cell(m.nr[0]) : '—'}</td>
                <td class="num">{pct(p.serviceS, p.observedS)}</td>
                <td><bdi dir="ltr">{p.allowedBands ? bandsText(supportedBands(p.allowedBands)) : '—'}</bdi></td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
      {#if pages > 1}
        <nav class="pager" aria-label={t('history.signal.pages')}>
          <button type="button" class="secondary" disabled={page === 0} onclick={() => (page -= 1)}>{t('history.signal.newer')}</button>
          <span>{t('history.signal.page', { n: num(page + 1), total: num(pages) })}</span>
          <button type="button" class="secondary" disabled={page >= pages - 1} onclick={() => (page += 1)}>{t('history.signal.older')}</button>
        </nav>
      {/if}
    </details>
  {/if}
</div>

<style>
  .wrap {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .head {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 4px 6px;
  }
  .seg {
    align-self: flex-start;
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
  .legend .lte,
  .legend .nr {
    stroke-width: 2;
  }
  .legend .lte {
    stroke: var(--accent);
  }
  .legend .nr {
    stroke: var(--chart-2);
    stroke-dasharray: 6 4;
  }
  .legend .wash {
    fill: var(--accent);
    opacity: 0.2;
  }
  .muted {
    margin: 0;
    color: var(--muted);
  }
  summary {
    cursor: pointer;
    color: var(--accent);
    font-size: var(--fs-small);
  }
  .table-wrap {
    overflow-x: auto;
    margin-block-start: 8px;
  }
  table {
    inline-size: 100%;
    border-collapse: collapse;
    font-size: var(--fs-small);
  }
  th,
  td {
    padding-block: 4px;
    padding-inline: 8px;
    border-block-end: 1px solid var(--rule);
    text-align: start;
    white-space: nowrap;
  }
  th {
    color: var(--muted);
    font-weight: 500;
  }
  .pager {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 12px;
    margin-block-start: 8px;
    font-size: var(--fs-small);
  }
</style>
