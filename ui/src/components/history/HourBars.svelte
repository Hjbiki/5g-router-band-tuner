<script lang="ts">
  // Speed by time of day: one bar per local hour, the median download of the tests that ran in it,
  // with a line over the middle half. An hour with one test is hatched, since one test says little.
  // The hours run left to right, also in RTL, like every time axis in the app.
  import { app } from '../../lib/store.svelte'
  import { t, currentLanguage } from '../../lib/i18n'
  import { formatNumber } from '../../lib/format'
  import { mbps } from '../../lib/speedtest.svelte'
  import { MIN_HOUR_TESTS, byHourOfDay, history, niceAxis } from '../../lib/history.svelte'
  import { formatTime } from './TimeChart.svelte'

  const digits = $derived(app.settings?.digits ?? 'western')
  const lang = $derived(currentLanguage())
  const data = $derived(byHourOfDay(history.speeds))
  const hours = $derived(data.hours)
  const enough = $derived(data.valid >= MIN_HOUR_TESTS)
  const axis = $derived(niceAxis(Math.max(0, ...hours.map((h) => h.p75 ?? h.median ?? 0))))

  // Drawn at its real pixel width and a fixed height, like the time charts, so its text keeps
  // its CSS size at every window width.
  const H = 200
  const LEFT = 40
  const END = 8
  const TOP = 8
  const AXIS = 22
  /** Room each hour label needs. */
  const LABEL_PX = 48
  let width = $state(0)
  let wrap: HTMLDivElement | undefined = $state()
  $effect(() => {
    if (!wrap || typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(([entry]) => (width = entry.contentRect.width))
    ro.observe(wrap)
    return () => ro.disconnect()
  })
  // jsdom and the first frame report 0; draw at a sensible width until measured.
  const W = $derived(width > 0 ? width : 640)
  const plotW = $derived(Math.max(24, W - LEFT - END))
  const bottom = H - AXIS
  const slot = $derived(plotW / 24)
  const barW = $derived(slot * 0.6)
  // Every 3 hours, or every 6 when the window is too narrow for that.
  const labelEvery = $derived(slot * 3 >= LABEL_PX ? 3 : 6)
  const cx = (hour: number) => LEFT + slot * hour + slot / 2
  const y = (v: number) => TOP + (1 - Math.min(axis.max, Math.max(0, v)) / axis.max) * (bottom - TOP)

  // A fixed day: only the hour matters, in the user's own clock format and digits.
  const hourLabel = (hour: number) => formatTime(new Date(2026, 0, 1, hour).getTime(), lang, digits)
  const num = (n: number) => formatNumber(n, { digits })
  const barText = (h: (typeof hours)[number]) =>
    h.n === 0
      ? t('history.hours.noTests', { hour: hourLabel(h.hour) })
      : t('history.hours.bar', { hour: hourLabel(h.hour), value: mbps(h.median, digits), n: num(h.n) })
  const hatch = $props.id()
</script>

<div class="body" data-testid="hours">

  {#if !enough}
    <p class="muted" data-testid="hours-few">{t('history.empty.fewTests', { min: num(MIN_HOUR_TESTS), n: num(data.valid) })}</p>
  {:else}
    <p class="muted small">{t('history.hours.caption', { n: num(data.valid) })}</p>
    <ul class="legend" aria-hidden="true">
      <li><svg width="12" height="12"><rect class="bar" x="2" y="2" width="8" height="10" /></svg>{t('history.hours.legendBar')}</li>
      <li><svg width="12" height="12"><line class="whisker" x1="6" x2="6" y1="1" y2="11" /></svg>{t('history.hours.legendWhisker')}</li>
      <li>
        <svg width="12" height="12"><rect class="hatched" fill="url(#{hatch}-key)" x="2" y="2" width="8" height="10" /><defs
            ><pattern id="{hatch}-key" width="4" height="4" patternUnits="userSpaceOnUse" patternTransform="rotate(45)"
              ><line class="hatch-line" x1="0" y1="0" x2="0" y2="4" /></pattern
            ></defs
          ></svg
        >{t('history.hours.legendFew')}
      </li>
    </ul>
    <div class="wrap" bind:this={wrap}>
      <svg width={W} height={H} viewBox="0 0 {W} {H}" role="img" aria-label={t('history.hours.chartLabel')} data-testid="hour-bars">
        <defs>
          <pattern id={hatch} width="6" height="6" patternUnits="userSpaceOnUse" patternTransform="rotate(45)">
            <line class="hatch-line" x1="0" y1="0" x2="0" y2="6" />
          </pattern>
        </defs>
        <g aria-hidden="true">
          {#each axis.ticks as tk (tk)}
            <line class={tk === 0 ? 'zero' : 'grid'} x1={LEFT} x2={W - END} y1={y(tk)} y2={y(tk)} />
            <text class="ytick" x={LEFT - 6} y={y(tk)} dy="0.32em">{num(tk)}</text>
          {/each}
          {#each hours as h (h.hour)}
            {#if h.hour % labelEvery === 0}<text class="xtick" x={cx(h.hour)} y={H - 6}>{hourLabel(h.hour)}</text>{/if}
          {/each}
        </g>
        {#each hours as h (h.hour)}
          {#if h.median !== null}
            <g class="hour" data-hour={h.hour} data-few={h.few}>
              <title>{barText(h)}</title>
              <!-- CSS would win over a fill attribute, so the hatched bar has no CSS fill. -->
              <rect
                class={h.few ? 'hatched' : 'bar'}
                fill={h.few ? `url(#${hatch})` : undefined}
                x={cx(h.hour) - barW / 2}
                y={y(h.median)}
                width={barW}
                height={Math.max(1, bottom - y(h.median))}
              />
              {#if h.n >= 2 && h.p25 !== null && h.p75 !== null && h.p75 > h.p25}
                <line class="whisker" x1={cx(h.hour)} x2={cx(h.hour)} y1={y(h.p25)} y2={y(h.p75)} />
                <line class="whisker" x1={cx(h.hour) - 3} x2={cx(h.hour) + 3} y1={y(h.p75)} y2={y(h.p75)} />
                <line class="whisker" x1={cx(h.hour) - 3} x2={cx(h.hour) + 3} y1={y(h.p25)} y2={y(h.p25)} />
              {/if}
            </g>
          {/if}
        {/each}
      </svg>
    </div>

    <details>
      <summary>{t('history.table.show')}</summary>
      <div class="table-wrap">
        <table>
          <caption class="sr-only">{t('history.hours.chartLabel')}</caption>
          <thead>
            <tr>
              <th scope="col">{t('history.hours.col.hour')}</th>
              <th scope="col" class="n">{t('history.hours.col.tests')}</th>
              <th scope="col" class="n">{t('history.hours.col.median')} <span class="th-unit">{t('speedtest.unit.mbps')}</span></th>
              <th scope="col" class="n">{t('history.hours.col.middle')} <span class="th-unit">{t('speedtest.unit.mbps')}</span></th>
            </tr>
          </thead>
          <tbody>
            {#each hours.filter((h) => h.n > 0) as h (h.hour)}
              <tr>
                <td><bdi dir="ltr">{hourLabel(h.hour)}</bdi></td>
                <td class="n">{num(h.n)}</td>
                <td class="n"><bdi dir="ltr">{mbps(h.median, digits)}</bdi></td>
                <td class="n">
                  {#if h.n >= 2}
                    {t('history.chart.span', { min: mbps(h.p25, digits), max: mbps(h.p75, digits) })}
                  {:else}—{/if}
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    </details>
  {/if}
</div>

<style>
  .body {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  /* Midnight on the left, also in RTL. */
  .wrap {
    direction: ltr;
  }
  svg[role='img'] {
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
  .bar {
    fill: var(--accent);
    opacity: 0.85;
  }
  .hatched {
    stroke: var(--accent);
    stroke-width: 1;
  }
  .hatch-line {
    stroke: var(--accent);
    stroke-width: 2;
  }
  .whisker {
    stroke: var(--text);
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
  th.n,
  td.n {
    text-align: end;
    font-variant-numeric: tabular-nums;
  }
  .th-unit {
    color: var(--faint);
  }
</style>
