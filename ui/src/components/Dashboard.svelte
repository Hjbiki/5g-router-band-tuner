<script lang="ts">
  import { onMount } from 'svelte'
  import ScanSearch from '@lucide/svelte/icons/scan-search'
  import SlidersHorizontal from '@lucide/svelte/icons/sliders-horizontal'
  import { app } from '../lib/store.svelte'
  import { t } from '../lib/i18n'
  import { isolate, updatedAgo } from '../lib/format'
  import { diagnose, type Line } from '../lib/diagnosis'
  import SignalCard from './SignalCard.svelte'
  import { fiveGReason } from '../lib/diagnosis'
  import ConnectionCard from './ConnectionCard.svelte'
  import LastSpeedTest from './LastSpeedTest.svelte'
  import CellDetails from './CellDetails.svelte'
  import { setRange } from '../lib/history.svelte'
  import { needsLogin } from '../lib/login'
  import { changeOpen } from '../lib/bands'
  import { scanActive } from '../lib/scan.svelte'
  import LoginNeeded from './ui/LoginNeeded.svelte'
  import NoServiceHelp from './NoServiceHelp.svelte'
  import SignalSpectrum from './spectrum/SignalSpectrum.svelte'

  const snap = $derived(app.snapshot)
  const digits = $derived(app.settings?.digits ?? 'western')
  const lte = $derived(snap?.signal?.lte ?? null)
  const nr = $derived(snap?.signal?.nr ?? null)
  const loggedOut = $derived(needsLogin(snap?.session))
  /** The Spectrum design has its own body for this page; the header and the notices are shared. */
  const spectrum = $derived(app.settings?.design === 'spectrum')

  const diagnosis = $derived(
    snap
      ? diagnose({ session: snap.session, status: snap.status, signal: snap.signal, history: app.history })
      : null,
  )
  // Rat names and numbers in the sentences stay left-to-right inside Arabic text.
  const say = (l: Line) =>
    t(l.key, l.vars ? Object.fromEntries(Object.entries(l.vars).map(([k, v]) => [k, isolate(v)])) : undefined)

  let now = $state(Date.now())
  onMount(() => {
    const id = setInterval(() => (now = Date.now()), 1000)
    return () => clearInterval(id)
  })
  const updated = $derived(updatedAgo(snap?.capturedAtMs, now, digits))

  /** "Room to improve": the History page's answer, which bands have been fastest over the last 30 days. */
  function seeFastest() {
    app.historyFocus = 'answer'
    void setRange({ kind: '30d' })
    app.tab = 'history'
  }
</script>

<div class="dash">
  <div class="title-row">
    <div class="heading">
      <div class="title-line">
        <h1>{t('dashboard.title')}</h1>
        {#if updated}<span class="updated" data-testid="updated">{updated}</span>{/if}
      </div>
      <p class="diagnosis" aria-live="polite" data-testid="diagnosis">
        {#if diagnosis}
          <b>{say(diagnosis.main)}</b>
          {#if diagnosis.secondary}<span class="secondary">{say(diagnosis.secondary)}</span>{/if}
          {#if diagnosis.rule === 'fair'}
            <button type="button" class="link" data-testid="history-link" onclick={seeFastest}>{t('history.link.signal')}</button>
          {/if}
        {:else}
          <b>{t('diagnosis.waiting')}</b>
        {/if}
      </p>
    </div>
    <div class="actions">
      <button type="button" class="action" onclick={() => (app.tab = 'bands')}>
        <SlidersHorizontal size={16} aria-hidden="true" />{t('dashboard.changeBands')}
      </button>
      <button type="button" class="action primary" onclick={() => (app.tab = 'scan')}>
        <ScanSearch size={16} aria-hidden="true" />{t('dashboard.findFaster')}
      </button>
    </div>
  </div>
  <!-- During a band change or a scan, no service is expected and the banner already covers it. -->
  {#if diagnosis?.rule === 'noService' && !changeOpen(snap?.safeApply) && !scanActive()}<NoServiceHelp />{/if}
  {#if loggedOut}<LoginNeeded what="login.needed.signal" />{/if}
  {#if spectrum && !loggedOut}
    <SignalSpectrum {lte} {nr} />
  {:else}
  <div class="cards" class:alone={loggedOut}>
    {#if !loggedOut}
    <SignalCard rat="lte" signal={lte} history={app.history} paired={nr !== null} bars={snap?.status?.bars} />
    <SignalCard
      rat="nr"
      signal={nr}
      history={app.history}
      paired={lte !== null}
      bars={snap?.status?.nrBars}
      noFiveG={lte !== null ? fiveGReason(snap?.status?.endc, snap?.status?.endcRestricted) : null}
    />
    {/if}
    <div class="side">
      <ConnectionCard />
      <LastSpeedTest />
    </div>
  </div>
  {#if !loggedOut}<CellDetails {lte} {nr} />{/if}
  {/if}
</div>

<style>
  .dash {
    display: flex;
    flex-direction: column;
    gap: var(--gap);
  }
  .title-row {
    display: flex;
    flex-wrap: wrap;
    align-items: flex-end;
    gap: 14px;
  }
  .heading {
    flex: 1;
    min-inline-size: 240px;
  }
  .title-line {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 2px 12px;
  }
  h1 {
    margin: 0;
    font-family: var(--font-heading);
    font-weight: 700;
    font-size: var(--fs-title);
    line-height: 32px;
  }
  /* Spectrum: the verdict is the page's headline, and the page name steps back to a label. */
  :global(:root[data-design='spectrum']) h1 {
    font-size: var(--fs-small);
    font-weight: 500;
    line-height: 18px;
    color: var(--muted);
  }
  .diagnosis {
    margin-block: 2px 0;
    margin-inline: 0;
    font-size: var(--fs-lead);
    max-inline-size: var(--measure);
  }
  /* The verdict is the page's answer, so it reads before the band codes below it. */
  .diagnosis b {
    display: block;
    font-family: var(--font-heading);
    font-size: var(--fs-verdict);
    font-weight: 600;
    line-height: 1.3;
  }
  .secondary {
    color: var(--muted);
  }
  button.link {
    all: unset;
    display: block;
    margin-block-start: 4px;
    color: var(--accent);
    font-size: var(--fs-small);
    text-decoration: underline;
    text-underline-offset: 2px;
    cursor: pointer;
  }
  button.link:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  .updated {
    color: var(--muted);
    font-size: var(--fs-small);
    font-variant-numeric: tabular-nums;
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  .action {
    block-size: var(--control-height);
    padding-block: 0;
    padding-inline: 14px;
    font-weight: 500;
    display: inline-flex;
    align-items: center;
    gap: 8px;
  }
  /* Two cards, with the side cards below them, until the window is wide enough for all three. */
  .cards {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: var(--gap);
  }
  .side {
    grid-column: 1 / -1;
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: var(--gap);
    min-inline-size: 0;
  }
  @media (min-width: 1360px) {
    .cards {
      grid-template-columns: minmax(0, 5fr) minmax(0, 4fr) minmax(280px, 3.6fr);
    }
    .side {
      grid-column: auto;
      grid-template-columns: minmax(0, 1fr);
      grid-template-rows: auto 1fr;
    }
    /* Logged out there are no signal cards beside them, so the two side cards share the row. */
    .cards.alone {
      grid-template-columns: minmax(0, 1fr);
    }
    .cards.alone .side {
      grid-template-columns: repeat(2, minmax(0, 1fr));
      grid-template-rows: auto;
    }
  }
  @media (max-width: 900px) {
    .cards,
    .side {
      grid-template-columns: minmax(0, 1fr);
    }
  }
</style>
