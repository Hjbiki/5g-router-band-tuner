<script lang="ts">
  import Wifi from '@lucide/svelte/icons/wifi'
  import { app } from '../lib/store.svelte'
  import { t } from '../lib/i18n'
  import { formatNumber, networkTypeLabel } from '../lib/format'
  import { bandsOf, supportedBands } from '../lib/bands'
  import { rangeText, wanKindShortKey } from '../lib/network'
  import Fact from './ui/Fact.svelte'
  import OperatorBadge from './ui/OperatorBadge.svelte'

  const snap = $derived(app.snapshot)
  const digits = $derived(app.settings?.digits ?? 'western')
  const operator = $derived(snap?.status?.operator ?? null)
  const network = $derived(networkTypeLabel(snap?.status?.networkType, snap?.signal) ?? '—')
  const scc = $derived(snap?.signal?.lte?.sccPci ?? null)
  const primary = $derived(app.settings?.automation.primaryBand ?? null)
  // The range only: the full address is on the Network page, on request.
  const wan = $derived(snap?.wan ?? null)
  // The LTE bands the router may use now: "all" when nothing is locked out.
  const allowed = $derived.by((): { all: boolean; bands: number[] } | null => {
    const cfg = snap?.config
    if (!cfg) return null
    const bands = bandsOf(cfg)
    const supported = snap?.capabilities ? supportedBands(snap.capabilities.supportedLte) : null
    const all = supported !== null && supported.length > 0 && supported.length === bands.length && supported.every((b, i) => b === bands[i])
    return { all, bands }
  })
</script>

<section class="card" aria-labelledby="connection-title">
  <h2 id="connection-title"><Wifi size={18} aria-hidden="true" />{t('dashboard.connection')}</h2>
  <dl>
    <Fact label={t('dashboard.operator')} help={t('metrics.help.operator')}>{#if operator}<OperatorBadge operator={{ ...operator, homePlmn: snap?.device?.homePlmn }} />{:else}—{/if}</Fact>
    <Fact label={t('dashboard.networkType')} term="NSA / SA" help={t('metrics.help.nsaSa')}>
      <bdi dir="ltr">{network}</bdi>
    </Fact>
    <Fact label={t('dashboard.carrierAggregation')} help={t('metrics.help.ca')}>
      {#if scc !== null}<bdi dir="ltr">PCI {formatNumber(scc, { digits })}</bdi>{:else}{t('dashboard.caOff')}{/if}
    </Fact>
    <Fact label={t('dashboard.allowedBands')} help={t('metrics.help.allowedBands')}>
      {#if allowed?.all}{t('dashboard.bandsAll')}{:else if allowed && allowed.bands.length}<bdi dir="ltr" data-testid="allowed-bands">{allowed.bands.map((b) => 'B' + b).join(' · ')}</bdi>{:else}—{/if}
    </Fact>
    <Fact label={t('dashboard.primaryBand')} help={t('bands.primaryHelp')}>
      {#if primary !== null}<bdi dir="ltr">B{primary}</bdi>{:else}{t('dashboard.primaryNone')}{/if}
    </Fact>
    <Fact label={t('network.wan.address')} help={t('network.wan.factHelp')}>
      <button type="button" class="link" data-testid="wan-fact" onclick={() => (app.tab = 'router')}>
        {#if wan?.range}<bdi dir="ltr" class="num">{rangeText(wan.range)}</bdi>{#if wan.kind}<span aria-hidden="true"> · </span><span>{t(wanKindShortKey(wan.kind))}</span>{/if}{:else if wan}{t('network.wan.none')}{:else}{t('network.wan.see')}{/if}
      </button>
    </Fact>
  </dl>
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
  dl {
    margin: 0;
  }
  .link {
    all: unset;
    color: var(--accent);
    font-weight: 500;
    text-decoration: underline;
    cursor: pointer;
  }
  .link:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
</style>
