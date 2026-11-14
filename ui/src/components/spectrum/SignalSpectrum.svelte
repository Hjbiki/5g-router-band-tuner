<script lang="ts">
  // The Signal page in the Spectrum design: the speed and a picture of the link first, then the
  // six measurements in one table. Same data and actions as the classic cards.
  import { onDestroy, onMount, untrack } from 'svelte'
  import type { LteSignal } from '../../lib/bindings/LteSignal'
  import type { NrSignal } from '../../lib/bindings/NrSignal'
  import { app } from '../../lib/store.svelte'
  import { t, type MessageKey } from '../../lib/i18n'
  import { LTE_BAND_INFO, changeOpen } from '../../lib/bands'
  import { THRESHOLDS, formatFigure, formatMeasure, formatNumber, isolate, qualityOf } from '../../lib/format'
  import { scanActive } from '../../lib/scan.svelte'
  import { cancelTest, ensureSpeedTest, mbps, ms, speed, stageKey, startTest, whenLabel } from '../../lib/speedtest.svelte'
  import ConnectionCard from '../ConnectionCard.svelte'
  import CellDetails from '../CellDetails.svelte'
  import SignalChart from '../SignalChart.svelte'

  let { lte, nr }: { lte: LteSignal | null; nr: NrSignal | null } = $props()

  onMount(() => {
    void ensureSpeedTest()
  })

  const digits = $derived(app.settings?.digits ?? 'western')
  const num = (n: number | null) => formatNumber(n, { digits })
  const mhz = (n: number | null) => isolate(formatMeasure(n, 'MHz', digits))

  // The speed test, as on the classic card: it runs in place.
  const scanning = $derived(scanActive())
  const last = $derived(speed.history[0] ?? null)
  const byScan = $derived(speed.running && scanning)
  const values = $derived(speed.running ? speed.live : last)
  const blocked = $derived(!speed.running && (changeOpen(app.snapshot?.safeApply) || scanning))
  const START_CAP_MS = 5000
  let starting = $state(false)
  let startCap: ReturnType<typeof setTimeout> | undefined
  function stopStarting() {
    starting = false
    if (startCap) clearTimeout(startCap)
    startCap = undefined
  }
  async function run() {
    if (starting) return
    starting = true
    startCap = setTimeout(stopStarting, START_CAP_MS)
    try {
      await startTest()
    } finally {
      if (speed.error) stopStarting()
    }
  }
  $effect(() => {
    if (starting && speed.running) untrack(stopStarting)
  })
  onDestroy(() => {
    if (startCap) clearTimeout(startCap)
  })

  // The link: one lane per radio, as thick as its channel is wide.
  const laneHeight = (bw: number | null) => (bw === null ? 8 : Math.max(4, Math.min(30, Math.round(bw * 0.3))))
  const lteMhz = $derived(lte?.band != null ? (LTE_BAND_INFO[lte.band]?.mhz ?? null) : null)
  const lanes = $derived([
    ...(nr
      ? [
          {
            id: 'nr',
            text: t('spectrum.lane5g', { band: isolate(nr.band ? `n${nr.band.primary}` : '5G'), mhz: mhz(nr.band?.dlMhz ?? null) }),
            wide: nr.dlBwMhz,
            main: true,
          },
        ]
      : []),
    ...(lte
      ? [
          {
            id: 'lte',
            text: t(nr ? 'spectrum.lane4gAnchor' : 'spectrum.lane4g', { band: isolate(lte.band != null ? `B${lte.band}` : '4G'), mhz: mhz(lteMhz) }),
            wide: lte.dlBwMhz,
            main: !nr,
          },
        ]
      : []),
  ])
  const pci = $derived(lte?.pci ?? nr?.pci ?? null)

  // The table: words first. Interference has its own words, so "good" never reads backwards.
  const INTERFERENCE: Record<string, MessageKey> = {
    excellent: 'spectrum.interference.excellent',
    good: 'spectrum.interference.good',
    fair: 'spectrum.interference.fair',
    poor: 'spectrum.interference.poor',
  }
  const cell = (metric: 'rsrp' | 'rsrq' | 'sinr', v: number | null, unit: 'dBm' | 'dB') => {
    const q = qualityOf(metric, v)
    return {
      q,
      word: q ? t(metric === 'sinr' ? INTERFERENCE[q] : (`quality.${q}` as MessageKey)) : '—',
      value: v === null ? '' : `${formatFigure(v, digits)} ${unit}`,
    }
  }
  const rows = $derived(
    [
      nr ? { id: 'nr', rat: '5G', band: nr.band ? `n${nr.band.primary}` : '', s: nr } : null,
      lte ? { id: 'lte', rat: '4G', band: lte.band != null ? `B${lte.band}` : '', s: lte } : null,
    ]
      .filter((r) => r !== null)
      .map((r) => ({ ...r, cells: [cell('rsrp', r.s.rsrp, 'dBm'), cell('rsrq', r.s.rsrq, 'dB'), cell('sinr', r.s.sinr, 'dB')] })),
  )

  // One trend only: the radio that holds the link.
  const trendRat = $derived(lte ? '4G' : '5G')
  const series = $derived(app.history.map((s) => ({ t: s.t, v: lte ? s.lteSinr : s.nrSinr })))
  const hasTrend = $derived(series.some((s) => s.v !== null && Number.isFinite(s.v)))
  const trendTitle = $derived(t('spectrum.trend', { rat: isolate(trendRat), n: isolate(num(5)) }))
</script>

<div class="spectrum">
  <div class="row">
    <section class="block hero" aria-label={t('spectrum.heroLabel')}>
      <div class="figures">
        <div class="fig lead">
          <span class="k">{t('speedtest.phase.download')}</span>
          <span class="v"><bdi dir="ltr" data-testid="spectrum-download">{mbps(values?.downloadMbps ?? null, digits)}</bdi> <span class="u">{t('speedtest.unit.mbps')}</span></span>
        </div>
        <div class="fig">
          <span class="k">{t('speedtest.phase.upload')}</span>
          <span class="v"><bdi dir="ltr">{mbps(values?.uploadMbps ?? null, digits)}</bdi> <span class="u">{t('speedtest.unit.mbps')}</span></span>
        </div>
        <div class="fig">
          <span class="k">{t('spectrum.ping')}</span>
          <span class="v"><bdi dir="ltr">{ms(values?.idlePingMs ?? null, digits)}</bdi> <span class="u">{t('speedtest.unit.ms')}</span></span>
        </div>
        <div class="when">
          {#if speed.running}
            <span role="status">{t(stageKey(speed.phase))}</span>
            {#if !byScan}<button type="button" class="link" onclick={cancelTest}>{t('speedtest.cancel')}</button>{/if}
          {:else}
            <span>{last ? t('spectrum.measured', { when: whenLabel(last.atMs, Date.now(), digits) }) : t('spectrum.noTest')}</span>
            <button type="button" class="link" onclick={run} disabled={blocked || starting}>{last ? t('speedtest.runAgain') : t('speedtest.last.run')}</button>
          {/if}
        </div>
      </div>

      {#if lanes.length > 0}
        <div class="link-picture" data-testid="link-picture">
          <div class="node">
            <svg width="40" height="48" viewBox="0 0 40 48" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"
              ><path d="M20 12 L8 46 M20 12 L32 46 M12 34 H28 M15 24 H25"></path><circle cx="20" cy="8" r="3"></circle><path d="M10 4 a12 12 0 0 0 0 9 M30 4 a12 12 0 0 1 0 9"></path></svg
            >
            <span class="name">{t('spectrum.tower')}</span>
            {#if pci !== null}<span class="sub">{t('spectrum.cell', { pci: isolate(num(pci)) })}</span>{/if}
          </div>
          <div class="lanes">
            {#each lanes as lane (lane.id)}
              <div class="lane">
                <div class="lane-text">
                  <span>{lane.text}</span>
                  {#if lane.wide !== null}<span class="k">{t('spectrum.wide', { mhz: mhz(lane.wide) })}</span>{/if}
                </div>
                <div class="bar" class:main={lane.main} style:block-size="{laneHeight(lane.wide)}px"></div>
              </div>
            {/each}
          </div>
          <div class="node">
            <svg width="48" height="48" viewBox="0 0 48 48" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"
              ><rect x="6" y="26" width="36" height="14" rx="3"></rect><path d="M14 33 h0.1 M20 33 h0.1 M34 26 V10"></path><path d="M28 12 a8 8 0 0 1 12 0"></path></svg
            >
            <span class="name">{t('spectrum.router')}</span>
            {#if app.snapshot?.device}<span class="sub"><bdi dir="ltr">{app.snapshot.device.model}</bdi></span>{/if}
          </div>
        </div>
        <p class="note">{t('spectrum.laneHint')}</p>
      {:else}
        <p class="empty">{t('spectrum.noLink')}</p>
      {/if}
    </section>

    <div class="side"><ConnectionCard /></div>
  </div>

  {#if rows.length > 0}
    <div class="row">
      <section class="block table" aria-labelledby="spectrum-signal">
        <h2 id="spectrum-signal">{t('spectrum.signalNow')}</h2>
        <div class="scroll">
          <table>
            <thead>
              <tr>
                <td></td>
                <th scope="col">{t('metrics.strength')}</th>
                <th scope="col">{t('metrics.quality')}</th>
                <th scope="col">{t('metrics.interference')}</th>
              </tr>
            </thead>
            <tbody>
              {#each rows as r (r.id)}
                <tr>
                  <th scope="row"><bdi dir="ltr">{r.rat}</bdi> <bdi dir="ltr" class="k">{r.band}</bdi></th>
                  {#each r.cells as c, i (i)}
                    <td>
                      <span class="cell">
                        <span class="word" style:--q={c.q ? `var(--q-${c.q})` : 'var(--muted)'}>{c.word}</span>
                        <bdi dir="ltr" class="k">{c.value}</bdi>
                      </span>
                    </td>
                  {/each}
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      </section>

      <section class="block side trend" aria-labelledby="spectrum-trend">
        <h2 id="spectrum-trend">{trendTitle}</h2>
        {#if hasTrend}
          <div class="chart"><SignalChart samples={series} label={trendTitle} reference={THRESHOLDS.sinr[1]} {digits} /></div>
          <p class="note">{t('spectrum.trendHint')}</p>
        {:else}
          <p class="note">{t('dashboard.chartEmpty')}</p>
        {/if}
      </section>
    </div>
  {/if}

  <CellDetails {lte} {nr} />
</div>

<style>
  .spectrum {
    display: flex;
    flex-direction: column;
    gap: var(--gap);
  }
  .row {
    display: flex;
    flex-wrap: wrap;
    gap: var(--gap);
  }
  .block {
    flex: 2 1 520px;
    min-inline-size: 0;
    box-sizing: border-box;
    background: var(--surface);
    border-radius: 12px;
    padding-block: 24px;
    padding-inline: 28px;
  }
  .side {
    flex: 1 1 300px;
    min-inline-size: 0;
    display: flex;
    flex-direction: column;
  }
  .side :global(.card) {
    flex: 1;
  }
  h2 {
    margin-block: 0 12px;
    margin-inline: 0;
    font-size: var(--fs-body);
    font-weight: 600;
    line-height: 22px;
  }
  .hero {
    display: flex;
    flex-direction: column;
    gap: 24px;
  }
  .figures {
    display: flex;
    flex-wrap: wrap;
    align-items: flex-end;
    gap: 12px 44px;
  }
  .fig {
    display: flex;
    flex-direction: column;
  }
  .k,
  .u,
  .note,
  .sub {
    color: var(--muted);
    font-size: var(--fs-small);
  }
  .u {
    font-size: var(--fs-body);
  }
  .v {
    font-size: 22px;
    line-height: 30px;
    font-weight: 500;
    font-variant-numeric: tabular-nums;
  }
  /* The one large figure on the page: the answer. */
  .lead .v {
    font-size: 36px;
    line-height: 44px;
    font-weight: 600;
  }
  .when {
    margin-inline-start: auto;
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    color: var(--muted);
    font-size: var(--fs-small);
  }
  button.link {
    all: unset;
    color: var(--accent);
    text-decoration: underline;
    text-underline-offset: 3px;
    cursor: pointer;
  }
  button.link:disabled {
    color: var(--faint);
    cursor: default;
  }
  button.link:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  .link-picture {
    display: grid;
    grid-template-columns: 92px minmax(0, 1fr) 112px;
    align-items: center;
    gap: 20px;
  }
  .node {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 4px;
    text-align: center;
  }
  .name {
    font-weight: 500;
  }
  .lanes {
    display: flex;
    flex-direction: column;
    gap: 18px;
  }
  .lane {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .lane-text {
    display: flex;
    flex-wrap: wrap;
    justify-content: space-between;
    gap: 4px 16px;
  }
  .bar {
    border-radius: 6px;
    background: var(--faint);
    min-block-size: 4px;
  }
  .bar.main {
    background: var(--accent);
  }
  .note,
  .empty {
    margin: 0;
  }
  .hero .note {
    margin-block-start: -8px;
  }
  .empty {
    font-size: var(--fs-section);
    font-weight: 500;
  }
  .scroll {
    overflow-x: auto;
  }
  table {
    inline-size: 100%;
    min-inline-size: 460px;
    border-collapse: collapse;
  }
  thead th {
    text-align: start;
    font-weight: 400;
    font-size: var(--fs-small);
    color: var(--muted);
    padding-block-end: 8px;
  }
  tbody th {
    text-align: start;
    font-weight: 600;
    white-space: nowrap;
    padding-inline-end: 24px;
  }
  tbody th .k {
    font-weight: 400;
  }
  tbody th,
  tbody td {
    border-block-start: 1px solid var(--rule);
    padding-block: 12px;
  }
  tbody td {
    padding-inline-end: 24px;
  }
  .cell {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 8px;
  }
  .word {
    color: var(--q);
    font-weight: 600;
  }
  .chart {
    block-size: 120px;
    display: flex;
    flex-direction: column;
  }
  @media (max-width: 620px) {
    .link-picture {
      grid-template-columns: minmax(0, 1fr);
    }
    .when {
      margin-inline-start: 0;
      align-items: flex-start;
    }
  }
</style>
