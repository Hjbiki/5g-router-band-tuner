<script lang="ts">
  // What a band chip's ⓘ shows: why a downlink-only band can't work alone, and the last recorded
  // result. Spans only, because it sits inside the popover's span.
  import { app } from '../lib/store.svelte'
  import { t } from '../lib/i18n'
  import { formatNumber } from '../lib/format'
  import { isDownlinkOnly, supportedBands } from '../lib/bands'
  import { mbps, ms, whenLabel } from '../lib/speedtest.svelte'
  import type { BandRecord } from '../lib/bandHistory'

  let { band, record }: { band: number; record: BandRecord | null } = $props()
  const digits = $derived(app.settings?.digits ?? 'western')
  const allowedLabel = (hex: string) =>
    supportedBands(hex)
      .map((b) => `B${b}`)
      .join(' + ')
</script>

<span class="last" data-testid="band-last-result">
  {#if isDownlinkOnly(band)}<span class="line" data-testid="band-add-on">{t('bands.last.addOn')}</span>{/if}
  <span class="title">{t('bands.last.title', { band: `B${band}` })}</span>
  {#if !record}
    <span class="line k">{t('bands.last.none')}</span>
  {:else}
    {#if record.kind === 'noService'}
      <span class="line">{t('bands.last.noService')}</span>
    {:else if record.kind === 'connected'}
      <span class="line">{t('bands.last.connected')}</span>
    {/if}
    <span class="rows">
      <span class="k">{t('bands.last.when')}</span><span>{whenLabel(record.atMs, Date.now(), digits)}</span>
      {#if record.kind === 'measured'}
        <span class="k">{t('speedtest.phase.download')}</span><span><bdi dir="ltr">{mbps(record.downloadMbps, digits)}</bdi> {t('speedtest.unit.mbps')}</span>
        <span class="k">{t('speedtest.phase.upload')}</span><span><bdi dir="ltr">{mbps(record.uploadMbps, digits)}</bdi> {t('speedtest.unit.mbps')}</span>
        <span class="k">{t('bands.last.ping')}</span><span><bdi dir="ltr">{ms(record.pingMs, digits)}</bdi> {t('speedtest.unit.ms')}</span>
        <span class="k">5G</span><span>{#if record.got5g}<bdi dir="ltr">{record.nrBand !== null ? `n${record.nrBand}` : '5G'}</bdi>{:else}{t('scan.no5g')}{/if}</span>
        {#if record.lteBand !== null}
          <span class="k">{t('bands.last.served')}</span><span><bdi dir="ltr">B{record.lteBand}{#if record.ltePci !== null}, PCI {formatNumber(record.ltePci, { digits })}{/if}</bdi></span>
        {/if}
        {#if record.allowed}
          <span class="k">{t('bands.last.allowed')}</span><span><bdi dir="ltr">{allowedLabel(record.allowed)}</bdi></span>
        {/if}
      {/if}
      <span class="k">{t('bands.last.source')}</span><span>{t(record.fromScan ? 'bands.last.fromScan' : 'bands.last.manual')}</span>
    </span>
  {/if}
</span>

<style>
  .last {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .line {
    display: block;
  }
  .title {
    font-weight: 600;
  }
  .rows {
    display: grid;
    grid-template-columns: auto auto;
    justify-content: start;
    gap: 2px 12px;
    font-variant-numeric: tabular-nums;
  }
  .k {
    color: var(--muted);
  }
</style>
