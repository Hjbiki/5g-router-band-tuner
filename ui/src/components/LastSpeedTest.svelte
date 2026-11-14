<script lang="ts">
  // The Signal page's speed test: runs in place, shows live progress and a short result, and
  // leads to the Speed test page for the rest.
  import ArrowDown from '@lucide/svelte/icons/arrow-down'
  import ArrowUp from '@lucide/svelte/icons/arrow-up'
  import ChevronRight from '@lucide/svelte/icons/chevron-right'
  import Gauge from '@lucide/svelte/icons/gauge'
  import Timer from '@lucide/svelte/icons/timer'
  import { onDestroy, onMount, untrack } from 'svelte'
  import { app } from '../lib/store.svelte'
  import { t } from '../lib/i18n'
  import { isolate } from '../lib/format'
  import { changeOpen } from '../lib/bands'
  import { scanActive } from '../lib/scan.svelte'
  import {
    cancelTest,
    diagnose,
    ensureSpeedTest,
    invalidReason,
    notComparableKey,
    mbps,
    ms,
    overallFraction,
    servingBands,
    servingDetail,
    speed,
    stageKey,
    startTest,
    whenLabel,
  } from '../lib/speedtest.svelte'

  onMount(() => {
    void ensureSpeedTest()
  })

  const digits = $derived(app.settings?.digits ?? 'western')
  const scanning = $derived(scanActive())
  const bandBusy = $derived(changeOpen(app.snapshot?.safeApply))
  const last = $derived(speed.history[0] ?? null)
  // An auto-scan's own tests send the same events; they aren't the user's to cancel.
  const byScan = $derived(speed.running && scanning)
  // Only a failure newer than the last result, and never a scan's own (it retries them).
  const showError = $derived(!!speed.error && !scanning && (speed.errorAtMs ?? 0) > (last?.atMs ?? 0))
  const values = $derived(speed.running ? speed.live : last)
  const blocked = $derived(!speed.running && (bandBusy || scanning))
  const overall = $derived(overallFraction(speed.phase, speed.fraction))

  // From the click until the test is seen running: run_speed_test returns before 'started'
  // arrives. A refusal releases it at once, and a cap covers a 'started' that never comes.
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
      // startTest records a refusal in speed.error rather than throwing.
      if (speed.error) stopStarting()
    }
  }
  $effect(() => {
    if (starting && speed.running) untrack(stopStarting)
  })
  onDestroy(() => {
    if (startCap) clearTimeout(startCap)
  })

  const where = $derived.by(() => {
    if (!last) return ''
    const detail = servingDetail(last.serving)
    return t('speedtest.last.when', {
      when: whenLabel(last.atMs, Date.now(), digits),
      serving: isolate(servingBands(last.serving)) + (detail ? `, ${detail}` : ''),
    })
  })
  const subLine = $derived.by(() => {
    if (speed.running) return speed.start ? t('speedtest.home.using', { provider: t(`speedtest.provider.${speed.start.provider}`) }) : ''
    if (!last) return t('speedtest.last.noneDetail')
    if (showError) return t('speedtest.home.previous', { when: whenLabel(last.atMs, Date.now(), digits) })
    return where
  })
  const title = $derived(
    speed.running ? t('speedtest.testingNow') : last ? t('speedtest.last.title') : t('speedtest.last.none'),
  )
  const runLabel = $derived(
    showError ? t('speedtest.home.tryAgain') : last ? t('speedtest.runAgain') : t('speedtest.last.run'),
  )
</script>

<section class="card" aria-labelledby="last-speed-title">
  <div class="head">
    <h2 id="last-speed-title"><Gauge size={18} aria-hidden="true" />{title}</h2>
    <button type="button" class="link more" onclick={() => (app.tab = 'speedtest')}>
      {t('speedtest.home.moreDetails')}<ChevronRight class="chev" size={14} aria-hidden="true" />
    </button>
  </div>
  {#if subLine}<p class="k">{subLine}</p>{/if}

  {#if showError}
    <div class="err" role="alert">
      <p class="err-title">{t('speedtest.home.failed')}</p>
      <p>{speed.error}</p>
    </div>
  {/if}

  <div class="tiles">
    <div class="tile lead">
      <span class="label"><ArrowDown class="dir" size={14} aria-hidden="true" />{t('speedtest.phase.download')}</span>
      <span class="value" data-testid="home-download"
        ><bdi dir="ltr">{mbps(values?.downloadMbps ?? null, digits)}</bdi> <span class="k">{t('speedtest.unit.mbps')}</span></span
      >
    </div>
    <div class="tile">
      <span class="label"><ArrowUp class="dir" size={14} aria-hidden="true" />{t('speedtest.phase.upload')}</span>
      <span class="value" data-testid="home-upload"
        ><bdi dir="ltr">{mbps(values?.uploadMbps ?? null, digits)}</bdi> <span class="k">{t('speedtest.unit.mbps')}</span></span
      >
    </div>
    <div class="tile">
      <span class="label"><Timer class="dir" size={14} aria-hidden="true" />{t('speedtest.home.ping')}</span>
      <span class="value" data-testid="home-ping"
        ><bdi dir="ltr">{ms(values?.idlePingMs ?? null, digits)}</bdi> <span class="k">{t('speedtest.unit.ms')}</span></span
      >
    </div>
  </div>

  {#if speed.running}
    <div
      class="bar"
      class:opaque={overall === null}
      role="progressbar"
      aria-label={t('speedtest.home.progressLabel')}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={overall === null ? undefined : Math.round(overall * 100)}
    >
      <i style:inline-size={overall === null ? '100%' : `${overall * 100}%`}></i>
    </div>
    <p class="k" role="status">{t(stageKey(speed.phase))}</p>
  {:else if last && !showError}
    <p class="verdict">{diagnose(last, digits).headline}</p>
    {#if invalidReason(last)}<p class="k">{t(notComparableKey(last))}</p>{/if}
  {/if}
  {#if speed.cancelled && !speed.running}<p class="k" role="status">{t('speedtest.cancelled')}</p>{/if}

  <div class="actions">
    {#if byScan}
      <p class="note">{t('speedtest.home.byScan')}</p>
      <button type="button" class="link" onclick={() => (app.tab = 'scan')}>{t('speedtest.home.openScan')}</button>
    {:else if speed.running}
      <button type="button" class="secondary" onclick={cancelTest}>{t('speedtest.cancel')}</button>
    {:else}
      <button
        type="button"
        class="primary"
        onclick={run}
        disabled={blocked || starting}
        aria-describedby={blocked ? 'last-speed-blocked' : undefined}
      >
        <Gauge size={16} aria-hidden="true" />{runLabel}
      </button>
      {#if blocked}
        <p class="note" id="last-speed-blocked">{t(scanning ? 'scan.blockedRunning' : 'speedtest.error.busy')}</p>
      {/if}
    {/if}
  </div>
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
    gap: 10px;
    min-inline-size: 0;
  }
  p {
    margin: 0;
  }
  .head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    flex-wrap: wrap;
    gap: 4px 12px;
  }
  .head h2 {
    margin: 0;
  }
  .k {
    color: var(--muted);
    font-size: var(--fs-small);
  }
  .card :global(.dir) {
    color: var(--muted);
    vertical-align: -2px;
  }
  .tiles {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(84px, 1fr));
    gap: 8px;
  }
  .tile {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding-block: 8px;
    padding-inline: 10px;
    border-radius: var(--radius-control);
    background: var(--raised);
    min-inline-size: 0;
  }
  .tile .label {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    color: var(--muted);
    font-size: var(--fs-small);
  }
  .tile .value {
    font-weight: 600;
    font-size: var(--fs-value);
    white-space: nowrap;
  }
  .tile.lead .value {
    font-size: 22px;
  }
  .tile .value .k {
    font-weight: 400;
  }
  .verdict {
    font-weight: 500;
  }
  .bar {
    block-size: 4px;
    border-radius: 2px;
    background: var(--q-track);
    overflow: hidden;
  }
  .bar i {
    display: block;
    block-size: 4px;
    background: var(--accent);
    transition: inline-size 0.2s linear;
  }
  .bar.opaque i {
    opacity: 0.35;
  }
  .note {
    margin: 0;
    padding-block: 8px;
    padding-inline: 10px;
    border-radius: var(--radius-control);
    background: var(--raised);
    font-size: var(--fs-small);
  }
  .err {
    margin: 0;
    padding-block: 10px;
    padding-inline: 12px;
    border-radius: var(--radius-control);
    background: var(--pill-negative-bg);
    color: var(--pill-negative-ink);
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .err-title {
    font-weight: 600;
  }
  .actions {
    margin-block-start: auto;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 8px;
  }
  .actions button.primary,
  .actions button.secondary {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    min-block-size: var(--control-height);
    padding-inline: 14px;
  }
  .actions button.secondary {
    background: var(--secondary);
    border-color: var(--secondary);
    font-weight: 500;
  }
  button.link {
    all: unset;
    display: inline-flex;
    align-items: center;
    gap: 2px;
    color: var(--accent);
    font-weight: 500;
    font-size: var(--fs-small);
    text-decoration: underline;
    text-underline-offset: 2px;
    cursor: pointer;
  }
  button.link:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  /* The chevron points the reading direction. */
  :global([dir='rtl']) .more :global(.chev) {
    transform: scaleX(-1);
  }
</style>
