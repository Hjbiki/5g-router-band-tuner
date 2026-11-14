<script lang="ts">
  // The History page's three cards under the answer: how the signal held up, the typical speed,
  // and whether automatic tests are filling the page in. Each says one thing in plain words.
  import Activity from '@lucide/svelte/icons/activity'
  import Gauge from '@lucide/svelte/icons/gauge'
  import Clock from '@lucide/svelte/icons/clock'
  import { tick } from 'svelte'
  import { app } from '../../lib/store.svelte'
  import { t } from '../../lib/i18n'
  import { formatNumber } from '../../lib/format'
  import { mbps } from '../../lib/speedtest.svelte'
  import { history, medianOf, signalHealth, speedSource, validSpeed } from '../../lib/history.svelte'
  import InfoTip from '../ui/InfoTip.svelte'

  const digits = $derived(app.settings?.digits ?? 'western')
  const num = (n: number) => formatNumber(n, { digits })
  const pct = (n: number | null) => (n === null ? '—' : t('history.cards.percent', { n: num(Math.round(n)) }))

  const health = $derived(signalHealth(history.signal?.points ?? [], history.events, history.signal?.bucketMs))
  const valid = $derived(history.speeds.filter(validSpeed))
  const typical = $derived(medianOf(valid.map((r) => r.downloadMbps)))
  const auto = $derived(app.settings?.history.autoSpeedTest ?? null)
  const autoRan = $derived(history.speeds.filter((r) => speedSource(r, history.scanPeriods) === 'auto').length)
  const reboots = $derived(health.rebootsMore ? `${num(health.reboots)}+` : num(health.reboots))

  async function openSettings() {
    app.tab = 'settings'
    await tick()
    document.getElementById('sec-history')?.scrollIntoView?.({ behavior: 'smooth', block: 'start' })
  }
</script>

<div class="cards" role="group" aria-label={t('history.cards.label')} class:stale={history.loading} data-testid="history-cards">
  <section class="tile" aria-labelledby="hc-signal" data-testid="card-signal">
    <div class="head">
      <h2 id="hc-signal"><Activity size={16} aria-hidden="true" />{t('history.signal.title')}</h2>
      <InfoTip label={t('history.cards.ownLabel')} text={t('history.cards.own')} />
    </div>
    <p class="big num">{pct(health.inServicePct)}</p>
    <p class="hint">{t('history.cards.inService')}</p>
    <ul>
      {#if health.on5gPct !== null}<li>{t('history.cards.on5g', { pct: pct(health.on5gPct) })}</li>{/if}
      <li data-testid="lost">{health.lost ? t('history.cards.lost', { n: num(health.lost) }) : t('history.cards.lostNone')}</li>
      <li data-testid="reboots">{t('history.cards.reboots', { n: reboots })}</li>
    </ul>
  </section>

  <section class="tile" aria-labelledby="hc-speed" data-testid="card-speed">
    <h2 id="hc-speed"><Gauge size={16} aria-hidden="true" />{t('history.speed.title')}</h2>
    <p class="big num">{typical === null ? '—' : t('speedtest.unit.speed', { n: mbps(typical, digits) })}</p>
    <p class="hint">{t('history.cards.typical')}</p>
    <ul>
      <li>{valid.length ? t('history.cards.tests', { n: num(valid.length) }) : t('history.cards.noTests')}</li>
    </ul>
  </section>

  <section class="tile" aria-labelledby="hc-auto" data-testid="card-auto">
    <h2 id="hc-auto"><Clock size={16} aria-hidden="true" />{t('history.cards.auto')}</h2>
    <p class="big">{t(auto?.enabled ? 'history.cards.autoOn' : 'history.cards.autoOff')}</p>
    {#if auto?.enabled}<p class="hint">{t('history.cards.autoEvery', { h: num(auto.everyH) })}</p>{/if}
    <ul>
      <li>{t('history.cards.autoRan', { n: num(autoRan) })}</li>
    </ul>
    {#if auto}
      <button type="button" class="link" onclick={() => void openSettings()}
        >{t(auto.enabled ? 'history.cards.autoSettings' : 'history.speed.turnOnAuto')}</button
      >
    {/if}
  </section>
</div>

<style>
  .cards {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
    gap: var(--gap);
    transition: opacity var(--t-fast) var(--ease);
  }
  .cards.stale {
    opacity: 0.6;
  }
  .tile {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 2px;
    padding-block: 12px;
    padding-inline: 16px;
    border: 1px solid var(--border);
    border-radius: var(--radius-card);
    background: var(--surface);
    box-shadow: var(--shadow-card);
  }
  .head {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  h2 {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    margin: 0;
    color: var(--muted);
    font-size: var(--fs-small);
    font-weight: 500;
  }
  h2 :global(svg) {
    color: var(--accent);
  }
  p {
    margin: 0;
  }
  .big {
    font-size: 22px;
    line-height: 30px;
    font-weight: 600;
  }
  .num {
    font-variant-numeric: tabular-nums;
  }
  .hint {
    color: var(--muted);
    font-size: var(--fs-small);
  }
  ul {
    display: flex;
    flex-direction: column;
    gap: 2px;
    margin-block: 6px 0;
    padding: 0;
    list-style: none;
    font-size: var(--fs-small);
  }
  button.link {
    all: unset;
    margin-block-start: 6px;
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
</style>
