<script lang="ts">
  import type { FiveGReason } from '../lib/diagnosis'
  import type { LteSignal } from '../lib/bindings/LteSignal'
  import type { NrSignal } from '../lib/bindings/NrSignal'
  import type { Sample } from '../lib/store.svelte'
  import { app } from '../lib/store.svelte'
  import RadioTower from '@lucide/svelte/icons/radio-tower'
  import { t } from '../lib/i18n'
  import { LTE_BAND_INFO } from '../lib/bands'
  import {
    THRESHOLDS,
    duplexLabel,
    formatFigure,
    formatMeasure,
    formatNumber,
    isolate,
    qualityOf,
    qualitySteps,
  } from '../lib/format'
  import InfoTip from './ui/InfoTip.svelte'
  import SignalChart from './SignalChart.svelte'
  import SignalBars from './ui/SignalBars.svelte'

  let {
    rat,
    signal,
    history,
    paired = false,
    bars = null,
    noFiveG = null,
  }: {
    rat: 'lte' | 'nr'
    signal: LteSignal | NrSignal | null
    history: Sample[]
    /** Whether the other leg is up too: LTE is then the NSA anchor, NR runs as NSA. */
    paired?: boolean
    /** The router's own 0 to 5 score for this link. */
    bars?: number | null
    /** The router's explanation for a missing 5G leg, if it has one. */
    noFiveG?: FiveGReason | null
  } = $props()

  const digits = $derived(app.settings?.digits ?? 'western')
  const num = (n: number | null) => formatNumber(n, { digits })
  const measure = (n: number | null, unit: 'dBm' | 'dB' | 'MHz') => formatMeasure(n, unit, digits)
  const lte = $derived(rat === 'lte' ? (signal as LteSignal | null) : null)
  const nr = $derived(rat === 'nr' ? (signal as NrSignal | null) : null)
  const ratShort = $derived(rat === 'lte' ? '4G' : '5G')

  const ratName = $derived(t(rat === 'lte' ? 'dashboard.lte' : 'dashboard.nr'))
  const kicker = $derived(
    t(rat === 'lte' ? (paired ? 'dashboard.lteAnchor' : 'dashboard.lteOnly') : paired ? 'dashboard.nrNsa' : 'dashboard.nrSa'),
  )

  /** Threshold as "-100 dBm" in an LTR isolate, for the help sentences. */
  const th = (v: number, unit: 'dBm' | 'dB') => isolate(measure(v, unit))
  const metrics = $derived(
    signal
      ? ([
          {
            id: 'rsrp',
            name: 'metrics.strength',
            short: 'RSRP',
            v: signal.rsrp,
            unit: 'dBm',
            help: t('metrics.help.rsrp', { weak: th(THRESHOLDS.rsrp[2], 'dBm') }),
          },
          {
            id: 'rsrq',
            name: 'metrics.quality',
            short: 'RSRQ',
            v: signal.rsrq,
            unit: 'dB',
            help: t('metrics.help.rsrq', {
              excellent: th(THRESHOLDS.rsrq[0], 'dB'),
              poor: th(THRESHOLDS.rsrq[2], 'dB'),
            }),
          },
          {
            id: 'sinr',
            name: 'metrics.interference',
            short: 'SINR',
            v: signal.sinr,
            unit: 'dB',
            help: t('metrics.help.sinr', { good: th(THRESHOLDS.sinr[1], 'dB'), poor: th(THRESHOLDS.sinr[2], 'dB') }),
          },
        ] as const)
      : [],
  )

  const band = $derived(
    lte ? (lte.band != null ? `B${lte.band}` : '—') : nr?.band ? `n${nr.band.primary}` : '—',
  )
  const bandTitle = $derived(
    nr?.band && nr.band.alternates.length
      ? t('bands.alsoWithin', {
          bands: `n${nr.band.primary}`,
          others: nr.band.alternates.map((b) => `n${b}`).join(', '),
        })
      : undefined,
  )
  const bandMeta = $derived.by(() => {
    if (lte) {
      const info = lte.band != null ? LTE_BAND_INFO[lte.band] : undefined
      return { mhz: info?.mhz ?? null, duplex: info?.duplex ?? null }
    }
    return { mhz: nr?.band?.dlMhz ?? null, duplex: nr?.band?.duplex ?? null }
  })
  const bandSummary = $derived(
    [
      bandMeta.mhz != null ? measure(bandMeta.mhz, 'MHz') : null,
      bandMeta.duplex ? duplexLabel(bandMeta.duplex) : null,
      `PCI ${num(signal?.pci ?? null)}`,
    ]
      .filter(Boolean)
      .join(' · '),
  )
  const sinrSeries = $derived(history.map((s) => ({ t: s.t, v: rat === 'lte' ? s.lteSinr : s.nrSinr })))
  const hasChartData = $derived(sinrSeries.some((s) => s.v !== null && Number.isFinite(s.v)))
  const chartLabel = $derived(
    t('dashboard.chartLabel', { n: num(5), rat: ratShort, value: measure(sinrSeries.findLast((s) => s.v !== null)?.v ?? null, 'dB') }),
  )
</script>

<section class="card" aria-label={ratName}>
  <div class="top">
    <div class="kicker"><RadioTower size={15} aria-hidden="true" /><bdi>{kicker}</bdi></div>
    {#if signal}
      <div class="signal">
        <SignalBars value={bars} size={20} />
        <InfoTip label={t('metrics.barsHelpLabel')} text={t('metrics.help.bars')} />
      </div>
    {/if}
  </div>
  {#if signal}
    <div class="head">
      <span title={bandTitle}><bdi dir="ltr" class="band" data-testid="band">{band}</bdi></span>
      <bdi dir="ltr" class="meta">{bandSummary}</bdi>
      {#if lte && lte.sccPci != null}<span class="ca">{t('dashboard.carrierAggregation')}</span>{/if}
    </div>

    <div class="metrics">
      {#each metrics as m (m.id)}
        {@const q = qualityOf(m.id, m.v)}
        {@const steps = qualitySteps(q)}
        <div class="metric" style:--q={q ? `var(--q-${q})` : 'var(--muted)'}>
          <div class="name">
            <span class="title">{t(m.name)}</span>
            <span class="short">{m.short}</span>
          </div>
          <InfoTip label={t('metrics.whatIs', { name: m.short })} text={m.help} />
          <div class="rating">
            <span class="meter" aria-hidden="true">
              {#each [1, 2, 3, 4] as i (i)}<i class:on={i <= steps}></i>{/each}
            </span>
            {#if q}<span class="word">{t(`quality.${q}`)}</span>{/if}
          </div>
          <bdi dir="ltr" class="value"
            ><span data-testid={m.id}>{formatFigure(m.v, digits)}</span
            >{#if m.v !== null}<span class="unit"> {m.unit}</span>{/if}</bdi
          >
        </div>
      {/each}
    </div>

    <figure class="chart">
      <figcaption>
        {t('dashboard.chartCaption', { n: isolate(num(5)), good: isolate(formatNumber(THRESHOLDS.sinr[1], { digits })) })}
        <!-- A tip, not a note under the chart: both charts stay the same height, so their lines compare. -->
        {#if rat === 'nr'}<InfoTip label={t('metrics.whatIs', { name: 'SINR' })} text={t('dashboard.nrCached')} />{/if}
      </figcaption>
      {#if hasChartData}
        <SignalChart samples={sinrSeries} label={chartLabel} reference={THRESHOLDS.sinr[1]} {digits} />
      {:else}
        <p class="muted">{t('dashboard.chartEmpty')}</p>
      {/if}
    </figure>
  {:else}
    <p class="empty">{t(rat === 'lte' ? 'dashboard.noLte' : 'dashboard.noNr')}</p>
    <!-- The hint says the router is on 4G, so it only shows while it is. -->
    {#if rat === 'nr' && (paired || noFiveG)}<p class="muted" data-testid="no-5g-reason">{noFiveG ? t(`dashboard.noNrReason.${noFiveG}`) : t('dashboard.noNrHint')}</p>{/if}
  {/if}
</section>

<style>
  .card {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius-card);
    padding-block: var(--card-pad-block);
    padding-inline: var(--card-pad-inline);
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-inline-size: 0;
  }
  .top {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    min-block-size: 22px;
  }
  .signal {
    display: inline-flex;
    align-items: center;
    gap: 2px;
  }
  .kicker {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .kicker,
  .short,
  .unit,
  .meta,
  .ca,
  figcaption,
  .muted {
    color: var(--muted);
    font-size: var(--fs-small);
  }
  .head {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 8px;
  }
  .band {
    font-family: var(--font-heading);
    font-weight: 700;
    font-size: var(--fs-figure);
    line-height: 36px;
    font-variant-numeric: tabular-nums;
  }
  .meta {
    font-size: var(--fs-body);
    font-variant-numeric: tabular-nums;
  }
  .metrics {
    display: grid;
    grid-template-columns: max-content auto minmax(0, 1fr) 88px max-content;
  }
  .metric {
    grid-column: 1 / -1;
    display: grid;
    grid-template-columns: subgrid;
    align-items: center;
    column-gap: 6px;
    min-block-size: 44px;
    border-block-start: 1px solid var(--rule);
    padding-block: 4px;
  }
  .metric :global(.info) {
    align-self: start;
    margin-block-start: 0;
  }
  .name {
    display: flex;
    flex-direction: column;
  }
  .title {
    font-weight: 500;
  }
  .rating {
    grid-column: 4;
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .meter {
    display: flex;
    gap: 2px;
  }
  .meter i {
    display: block;
    inline-size: 20px;
    block-size: 6px;
    border-radius: 3px;
    background: var(--q-track);
    transition: background-color 240ms var(--ease);
  }
  .meter i.on {
    background: var(--q);
  }
  .word {
    font-size: var(--fs-body);
    font-weight: 600;
    color: var(--q);
  }
  .value {
    grid-column: 5;
    min-inline-size: 72px;
    text-align: end;
    font-size: var(--fs-value);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }
  .unit {
    margin-inline-start: 3px;
  }
  /* Takes whatever height the row gives the card, so cards in a row end level. */
  .chart {
    margin: 0;
    flex: 1;
    display: flex;
    flex-direction: column;
    min-block-size: 0;
  }
  .chart figcaption {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 2px 4px;
    min-block-size: 22px;
    margin-block-end: 4px;
  }
  .muted {
    margin: 0;
  }
  .empty {
    margin: 0;
    font-size: var(--fs-section);
    font-weight: 500;
  }
</style>
