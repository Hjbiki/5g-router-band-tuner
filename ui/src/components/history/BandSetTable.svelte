<script lang="ts">
  // Speed by band set: the valid speed tests in the range, grouped by the bands that were allowed.
  // There's no score; each column sorts on its own. "Try these bands" only fills the band editor.
  // The History page's More details section around it gives the heading.
  import ArrowUp from '@lucide/svelte/icons/arrow-up'
  import ArrowDown from '@lucide/svelte/icons/arrow-down'
  import { app, canTryBands, tryBands } from '../../lib/store.svelte'
  import { t, currentLanguage, type MessageKey } from '../../lib/i18n'
  import { formatNumber, isolate } from '../../lib/format'
  import { mbps, ms, whenLabel } from '../../lib/speedtest.svelte'
  import { supportedBands } from '../../lib/bands'
  import type { SpeedTestProvider } from '../../lib/bindings/SpeedTestProvider'
  import {
    FEW_BAND_TESTS,
    bandsText,
    byBandSet,
    history,
    sortBandSets,
    speedProviders,
    type BandSetRow,
    type BandSortKey,
  } from '../../lib/history.svelte'
  import BandTags from '../ui/BandTags.svelte'
  import InfoTip from '../ui/InfoTip.svelte'
  import { formatTime } from './TimeChart.svelte'

  const digits = $derived(app.settings?.digits ?? 'western')
  const lang = $derived(currentLanguage())
  const num = (n: number) => formatNumber(n, { digits })

  const providers = $derived(speedProviders(history.speeds))
  let chosen: SpeedTestProvider | null = $state(null)
  // The provider the user picked, while it still has tests; otherwise the first that does.
  const provider = $derived(chosen && providers.includes(chosen) ? chosen : (providers[0] ?? 'cloudflare'))

  let sortKey = $state<BandSortKey>('with5g')
  let sortDir = $state<'asc' | 'desc'>('desc')
  const rows = $derived(sortBandSets(byBandSet(history.speeds, provider), sortKey, sortDir))

  function sortBy(key: BandSortKey) {
    if (sortKey === key) sortDir = sortDir === 'asc' ? 'desc' : 'asc'
    else {
      sortKey = key
      // Lowest first is what you want of a ping; most first of everything else.
      sortDir = key === 'ping' ? 'asc' : 'desc'
    }
  }
  const ariaSort = (key: BandSortKey) => (sortKey === key ? (sortDir === 'asc' ? 'ascending' : 'descending') : 'none')

  const COLUMNS: { key: BandSortKey; label: MessageKey; unit?: 'mbps' | 'ms' }[] = [
    { key: 'tests', label: 'history.bands.col.tests' },
    { key: 'with5g', label: 'history.bands.col.with5g' },
    { key: 'down', label: 'history.bands.col.down', unit: 'mbps' },
    { key: 'up', label: 'history.bands.col.up', unit: 'mbps' },
    { key: 'ping', label: 'history.bands.col.ping', unit: 'ms' },
  ]

  const hourLabel = (hour: number) => formatTime(new Date(2026, 0, 1, hour).getTime(), lang, digits)
  const label = (r: BandSetRow) =>
    r.served !== null ? t('history.bands.served', { band: isolate(`B${r.served}`) }) : isolate(bandsText(supportedBands(r.bands)))

  const canTry = $derived(canTryBands())
</script>

<div class="wrap" data-testid="band-sets">
  <div class="intro">
    <p class="muted small">{t('history.bands.intro')}</p>
    <InfoTip label={t('history.bands.noScoreLabel')} text={t('history.bands.noScore')} />
  </div>

  {#if providers.length > 1}
    <div class="seg" role="radiogroup" aria-label={t('history.bands.provider')}>
      {#each providers as p (p)}
        <label class:on={provider === p}>
          <input type="radio" name="hs-bands-provider" checked={provider === p} onchange={() => (chosen = p)} />
          {t(`speedtest.provider.${p}`)}
        </label>
      {/each}
    </div>
  {/if}

  {#if rows.length === 0}
    <p class="muted">{t('history.bands.empty')}</p>
  {:else}
    <div class="table-wrap">
      <table>
        <caption class="sr-only">{t('history.bands.caption')}</caption>
        <thead>
          <tr>
            <th scope="col">{t('history.bands.col.bands')}</th>
            {#each COLUMNS as c (c.key)}
              <th scope="col" class="n" aria-sort={ariaSort(c.key)}>
                <button type="button" class="sort" onclick={() => sortBy(c.key)}>
                  {t(c.label)}{#if c.unit}{' '}<span class="th-unit">{t(`speedtest.unit.${c.unit}`)}</span>{/if}
                  {#if sortKey === c.key}
                    {#if sortDir === 'asc'}<ArrowUp size={12} aria-hidden="true" />{:else}<ArrowDown size={12} aria-hidden="true" />{/if}
                  {/if}
                </button>
              </th>
            {/each}
            <th scope="col">{t('history.bands.col.best')}</th>
            <th scope="col" aria-sort={ariaSort('last')}>
              <button type="button" class="sort" onclick={() => sortBy('last')}>
                {t('history.bands.col.last')}
                {#if sortKey === 'last'}
                  {#if sortDir === 'asc'}<ArrowUp size={12} aria-hidden="true" />{:else}<ArrowDown size={12} aria-hidden="true" />{/if}
                {/if}
              </button>
            </th>
            <th scope="col"><span class="sr-only">{t('history.bands.col.action')}</span></th>
          </tr>
        </thead>
        <tbody>
          {#each rows as r (r.key)}
            <tr data-key={r.key}>
              <th scope="row" class="bands">
                {#if r.served !== null}
                  <span title={t('history.bands.servedHint')}>{t('history.bands.served', { band: isolate(`B${r.served}`) })}</span>
                {:else}
                  <BandTags bands={r.bands} />
                {/if}
              </th>
              <td class="n">
                {num(r.tests)}
                {#if r.tests < FEW_BAND_TESTS}<span class="k sub">{t('history.bands.few')}</span>{/if}
              </td>
              <td class="n">{t('history.bands.of', { a: num(r.with5g), b: num(r.tests) })}</td>
              <td class="n strong-cell"><bdi dir="ltr">{mbps(r.medianDownMbps, digits)}</bdi></td>
              <td class="n"><bdi dir="ltr">{mbps(r.medianUpMbps, digits)}</bdi></td>
              <td class="n"><bdi dir="ltr">{ms(r.medianBusyPingMs, digits)}</bdi></td>
              <td>{r.bestHour === null ? '—' : hourLabel(r.bestHour)}</td>
              <td>{whenLabel(r.lastMs, Date.now(), digits)}</td>
              <td>
                <button
                  type="button"
                  class="secondary"
                  disabled={!canTry}
                  aria-label={t('history.bands.tryLabel', { bands: label(r) })}
                  onclick={() => tryBands(r.bands)}>{t('history.bands.try')}</button
                >
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
    <p class="muted small">{t('history.bands.tryHint')}</p>
  {/if}
  <div>
    <button type="button" class="link" onclick={() => (app.tab = 'scan')}>{t('history.bands.scanLink')}</button>
  </div>
</div>

<style>
  .wrap {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .intro {
    display: flex;
    align-items: center;
    gap: 6px;
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
  .table-wrap {
    overflow-x: auto;
  }
  /* In a narrow window the table scrolls inside the card, so the page doesn't. */
  @media (max-width: 760px) {
    .table-wrap {
      max-block-size: 360px;
      overflow: auto;
    }
  }
  table {
    inline-size: 100%;
    border-collapse: collapse;
  }
  th,
  td {
    padding-block: 8px;
    padding-inline: 0 12px;
    border-block-start: 1px solid var(--rule);
    text-align: start;
    vertical-align: top;
    white-space: nowrap;
  }
  thead th {
    border-block-start: 0;
    padding-block: 0 8px;
    font-weight: 400;
    font-size: var(--fs-small);
    color: var(--muted);
  }
  th.bands {
    font-weight: 500;
  }
  .n {
    text-align: end;
    font-variant-numeric: tabular-nums;
  }
  .th-unit {
    color: var(--faint);
  }
  .strong-cell {
    font-weight: 500;
  }
  .sub {
    display: block;
  }
  .k {
    color: var(--muted);
    font-size: var(--fs-small);
  }
  button.sort {
    all: unset;
    display: inline-flex;
    align-items: center;
    gap: 4px;
    cursor: pointer;
  }
  button.sort:hover {
    color: var(--text);
  }
  button.sort:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  button.secondary {
    min-block-size: var(--control-height);
    padding-inline: 12px;
    background: var(--secondary);
    border-color: var(--secondary);
    font-weight: 500;
  }
  button.link {
    all: unset;
    color: var(--accent);
    text-decoration: underline;
    text-underline-offset: 2px;
    cursor: pointer;
    font-size: var(--fs-small);
  }
  button.link:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  .muted {
    margin: 0;
    color: var(--muted);
  }
  .small {
    font-size: var(--fs-small);
  }
</style>
