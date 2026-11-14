<script lang="ts">
  // Speed by IP range: the valid speed tests in the range, grouped by the first two numbers of the
  // internet address they started on. Only those two numbers are ever stored. A range with too few
  // tests is listed but not compared. "Aim for this range" opens the Refresh IP dialog with the range
  // filled in; nothing changes until the user starts it there. It shows only where Refresh IP can
  // work (logged in, a router with a data switch), on compared rows, and not on the range in use.
  import { app } from '../../lib/store.svelte'
  import { t } from '../../lib/i18n'
  import { formatNumber, isolate } from '../../lib/format'
  import { mbps, ms, whenLabel } from '../../lib/speedtest.svelte'
  import { rangeText } from '../../lib/network'
  import { openIpRefresh } from '../../lib/ipRefresh.svelte'
  import type { SpeedTestProvider } from '../../lib/bindings/SpeedTestProvider'
  import { MIN_RANGE_TESTS, byIpRange, history, speedProviders } from '../../lib/history.svelte'
  import InfoTip from '../ui/InfoTip.svelte'

  const digits = $derived(app.settings?.digits ?? 'western')
  const num = (n: number) => formatNumber(n, { digits })

  const providers = $derived(speedProviders(history.speeds))
  let chosen: SpeedTestProvider | null = $state(null)
  const provider = $derived(chosen && providers.includes(chosen) ? chosen : (providers[0] ?? 'cloudflare'))
  const rows = $derived(byIpRange(history.speeds, provider))

  const aim = (range: string) => openIpRefresh(range)
  const canAim = $derived(app.snapshot?.session.kind === 'loggedIn' && (app.snapshot?.capabilities?.ipMethods?.length ?? 0) > 0)
  const nowRange = $derived(app.snapshot?.wan?.range ?? null)
</script>

<div class="intro">
  <p class="muted small">{t('history.ip.intro')}</p>
  <InfoTip label={t('history.ip.noteLabel')} text={t('history.ip.note')} />
</div>

{#if providers.length > 1}
  <div class="seg" role="radiogroup" aria-label={t('history.bands.provider')}>
    {#each providers as p (p)}
      <label class:on={provider === p}>
        <input type="radio" name="hs-ip-provider" checked={provider === p} onchange={() => (chosen = p)} />
        {t(`speedtest.provider.${p}`)}
      </label>
    {/each}
  </div>
{/if}

{#if rows.length === 0}
  <p class="muted">{t('history.ip.empty')}</p>
{:else}
  <div class="table-wrap">
    <table data-testid="ip-ranges">
      <caption class="sr-only">{t('history.ip.caption')}</caption>
      <thead>
        <tr>
          <th scope="col">{t('history.ip.col.range')}</th>
          <th scope="col" class="n">{t('history.bands.col.tests')}</th>
          <th scope="col" class="n">{t('history.bands.col.down')} <span class="th-unit">{t('speedtest.unit.mbps')}</span></th>
          <th scope="col" class="n">{t('history.bands.col.up')} <span class="th-unit">{t('speedtest.unit.mbps')}</span></th>
          <th scope="col" class="n">{t('history.bands.col.ping')} <span class="th-unit">{t('speedtest.unit.ms')}</span></th>
          <th scope="col">{t('history.bands.col.last')}</th>
          <th scope="col"><span class="sr-only">{t('history.bands.col.action')}</span></th>
        </tr>
      </thead>
      <tbody>
        {#each rows as r (r.range ?? '')}
          <tr data-range={r.range ?? 'unknown'}>
            <th scope="row">
              {#if r.range}<bdi dir="ltr" class="num">{rangeText(r.range)}</bdi>{:else}{t('history.ip.unknown')}{/if}
            </th>
            <td class="n">
              {num(r.tests)}
              {#if r.range && !r.compared}
                <span class="k sub" title={t('history.ip.notComparedHint', { n: num(MIN_RANGE_TESTS) })}>{t('history.ip.notCompared')}</span>
              {/if}
            </td>
            {#if r.compared}
              <td class="n strong-cell"><bdi dir="ltr">{mbps(r.medianDownMbps, digits)}</bdi></td>
              <td class="n"><bdi dir="ltr">{mbps(r.medianUpMbps, digits)}</bdi></td>
              <td class="n"><bdi dir="ltr">{ms(r.medianBusyPingMs, digits)}</bdi></td>
            {:else}
              <td class="n">—</td>
              <td class="n">—</td>
              <td class="n">—</td>
            {/if}
            <td>{whenLabel(r.lastMs, Date.now(), digits)}</td>
            <td>
              {#if r.range && r.range === nowRange}
                <span class="muted small" data-testid="ip-range-now">{t('history.ip.current')}</span>
              {:else if r.range && r.compared && canAim}
                <button
                  type="button"
                  class="secondary"
                  aria-label={t('history.ip.aimLabel', { range: isolate(rangeText(r.range)) })}
                  onclick={() => aim(r.range as string)}>{t('history.ip.aim')}</button
                >
              {/if}
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
{/if}

<style>
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
  tbody th {
    font-weight: 500;
  }
  .n {
    text-align: end;
    font-variant-numeric: tabular-nums;
  }
  .num {
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
  button.secondary {
    min-block-size: var(--control-height);
    padding-inline: 12px;
    background: var(--secondary);
    border-color: var(--secondary);
    font-weight: 500;
  }
  .muted {
    margin: 0;
    color: var(--muted);
  }
  .small {
    font-size: var(--fs-small);
  }
</style>
