<script lang="ts">
  // The History page: what the app recorded over a chosen time range, answer first. The answer card
  // says which bands have been fastest, three cards sum up the range, and the charts and tables sit
  // in More details sections that start closed. Dates, the CSV export and the retention are in the
  // footer.
  import ChartLine from '@lucide/svelte/icons/chart-line'
  import Activity from '@lucide/svelte/icons/activity'
  import Gauge from '@lucide/svelte/icons/gauge'
  import Clock from '@lucide/svelte/icons/clock'
  import Layers from '@lucide/svelte/icons/layers'
  import Network from '@lucide/svelte/icons/network'
  import ScrollText from '@lucide/svelte/icons/scroll-text'
  import FileDown from '@lucide/svelte/icons/file-down'
  import CalendarDays from '@lucide/svelte/icons/calendar-days'
  import { onMount, tick } from 'svelte'
  import { app } from '../lib/store.svelte'
  import { t, currentLanguage } from '../lib/i18n'
  import { formatNumber, isolate } from '../lib/format'
  import { rangeText } from '../lib/network'
  import {
    RANGE_KINDS,
    byIpRange,
    dayValue,
    emptyState,
    ensureHistory,
    fasterRange,
    hasIpRanges,
    history,
    loadHistory,
    parseDay,
    setRange,
    setSectionOpen,
    speedProviders,
    startOfDay,
    validCustom,
    type HistorySection,
    type RangeKind,
  } from '../lib/history.svelte'
  import HistoryAnswer from './history/HistoryAnswer.svelte'
  import HistoryCards from './history/HistoryCards.svelte'
  import DetailsSection from './history/DetailsSection.svelte'
  import SignalHistoryCard from './history/SignalHistoryCard.svelte'
  import SpeedHistoryCard from './history/SpeedHistoryCard.svelte'
  import HourBars from './history/HourBars.svelte'
  import BandSetTable from './history/BandSetTable.svelte'
  import IpRangeTable from './history/IpRangeTable.svelte'
  import EventLog from './history/EventLog.svelte'
  import HistoryExport from './history/HistoryExport.svelte'
  import { formatDay } from './history/TimeChart.svelte'

  onMount(() => {
    void ensureHistory()
    void loadHistory(0)
  })

  const digits = $derived(app.settings?.digits ?? 'western')
  const lang = $derived(currentLanguage())
  const overview = $derived(history.overview)
  const empty = $derived(emptyState(overview, history.signal, history.speeds))
  const num = (n: number) => formatNumber(n, { digits })

  // An empty range still shows what happened when anything was logged at all: a range without
  // signal (not logged in, router restarting) is often exactly what the log should explain.
  const logWhenEmpty = $derived((empty === 'range' || empty === 'fresh') && (overview?.events ?? 0) > 0)
  const ipRanges = $derived(hasIpRanges(history.speeds))

  // One status for the header. Recording follows the live session; the overview only says what
  // it was when the page loaded.
  type Status = 'recording' | 'paused' | 'loggedOut' | 'readOnly'
  const status = $derived.by((): Status | null => {
    if (!overview?.available) return null
    if (overview.readOnly) return 'readOnly'
    const session = app.snapshot?.session.kind
    if (session === 'loggedOut') return 'loggedOut'
    if (session ? session === 'loggedIn' : overview.recording) return 'recording'
    return 'paused'
  })

  // --- Deep links from other pages ---------------------------------------------------------------

  const SECTION_FOR = { bandSets: 'bandSets', ipRanges: 'ipRanges', latestChange: 'events' } as const

  // A page that sent the user here wants something an empty range doesn't show: drop the request,
  // so it doesn't fire on a later visit. Only once the new range has loaded, since the values
  // kept from an earlier visit say nothing about it.
  $effect(() => {
    if (!app.historyFocus || history.loading || empty === null) return
    if (app.historyFocus === 'latestChange' && logWhenEmpty) return
    app.historyFocus = null
  })

  // The newest band change: open What happened; the log itself waits for the range and focuses it.
  $effect(() => {
    if (app.historyFocus === 'latestChange' && (empty === null || logWhenEmpty)) setSectionOpen('events', true)
  })

  // The answer, or a closed section: open it (and keep it open), then bring it into view once.
  $effect(() => {
    const focus = app.historyFocus
    if (!focus || focus === 'latestChange' || history.loading || !overview || empty !== null) return
    app.historyFocus = null
    if (focus === 'ipRanges' && !ipRanges) return
    if (focus !== 'answer') setSectionOpen(SECTION_FOR[focus], true)
    const id = focus === 'answer' ? 'history-answer' : `hs-sec-${SECTION_FOR[focus]}`
    void tick().then(() => {
      const el = document.getElementById(id)
      el?.scrollIntoView?.({ behavior: 'smooth', block: 'start' })
      el?.focus({ preventScroll: true })
    })
  })

  // --- Range ---------------------------------------------------------------------------------------

  const RANGE_LABEL: Record<RangeKind, { key: 'history.range.day' | 'history.range.week' | 'history.range.month'; n: number }> = {
    '24h': { key: 'history.range.day', n: 24 },
    '7d': { key: 'history.range.week', n: 7 },
    '30d': { key: 'history.range.month', n: 30 },
  }

  // The custom days being typed; they only become the range once they make sense.
  const today = startOfDay(Date.now())
  let customFrom = $state(dayValue(history.range.kind === 'custom' ? history.range.fromMs : today - 6 * 86_400_000))
  let customTo = $state(dayValue(history.range.kind === 'custom' ? history.range.toMs : today))
  const customValid = $derived(validCustom(parseDay(customFrom), parseDay(customTo)))

  // Pick dates is open while its days are still being fixed, and stays shown for a custom range.
  let customOpen = $state(false)
  const showCustom = $derived(customOpen || history.range.kind === 'custom')
  const selected = $derived(customOpen ? 'custom' : history.range.kind)

  function pickKind(kind: RangeKind) {
    customOpen = false
    void setRange({ kind })
  }

  function pickDates() {
    if (showCustom) {
      document.getElementById('history-from')?.focus()
      return
    }
    applyCustom()
  }

  function applyCustom() {
    const fromMs = parseDay(customFrom)
    const toMs = parseDay(customTo)
    if (fromMs === null || toMs === null || !validCustom(fromMs, toMs)) {
      // Show the custom fields with the message; the loaded range stays as it was.
      customOpen = true
      return
    }
    customOpen = false
    void setRange({ kind: 'custom', fromMs, toMs })
  }

  const customLabel = $derived(
    history.range.kind === 'custom'
      ? t('history.range.showing', { from: formatDay(history.range.fromMs, lang, digits), to: formatDay(history.range.toMs, lang, digits) })
      : null,
  )

  let exportOpen = $state(false)
  const canExport = $derived(!!overview?.available && empty !== 'fresh')

  // The IP range section's line while closed: the range clearly ahead, by the first provider.
  const ipPreview = $derived.by(() => {
    const provider = speedProviders(history.speeds)[0]
    const range = provider ? fasterRange(byIpRange(history.speeds, provider)) : null
    return range ? t('history.ip.preview', { range: isolate(rangeText(range)) }) : null
  })

  const SECTIONS: { id: HistorySection; title: 'history.details.signal' | 'history.details.speeds' | 'history.hours.title' | 'history.details.bandSets' }[] = [
    { id: 'signal', title: 'history.details.signal' },
    { id: 'speeds', title: 'history.details.speeds' },
    { id: 'hours', title: 'history.hours.title' },
    { id: 'bandSets', title: 'history.details.bandSets' },
  ]
  const ICONS = { signal: Activity, speeds: Gauge, hours: Clock, bandSets: Layers }
</script>

<section class="page" aria-labelledby="history-title" aria-busy={history.loading}>
  <header>
    <div class="titlebar">
      <h1 id="history-title">{t('history.title')}</h1>
      {#if status}
        <span class="status {status}" data-testid="history-status">
          <span class="dot" aria-hidden="true"></span>{t(`history.status.${status}`)}
        </span>
        {#if status === 'paused' || status === 'loggedOut'}
          <button type="button" class="link" onclick={() => (app.tab = 'router')}>{t('history.status.connect')}</button>
        {/if}
      {/if}
    </div>
    {#if status === 'paused' || status === 'loggedOut'}<p class="lede">{t('history.status.pausedHint')}</p>{/if}
    {#if status === 'readOnly'}<p class="lede">{t('history.status.readOnlyHint')}</p>{/if}
  </header>

  <div class="ranges">
    <div class="seg" role="radiogroup" aria-label={t('history.range.label')}>
      {#each RANGE_KINDS as kind (kind)}
        <label class:on={selected === kind}>
          <input type="radio" name="history-range" checked={selected === kind} onchange={() => pickKind(kind)} />
          {t(RANGE_LABEL[kind].key, { n: num(RANGE_LABEL[kind].n) })}
        </label>
      {/each}
    </div>
    {#if customLabel && !customOpen}<span class="showing" data-testid="history-custom">{customLabel}</span>{/if}
  </div>

  {#if history.error}
    <div class="error" role="alert">
      <p>{t('history.error', { message: history.error })}</p>
      <button type="button" class="secondary" onclick={() => void loadHistory(0)}>{t('app.retry')}</button>
    </div>
  {/if}

  {#if !overview}
    {#if !history.error}
      <div class="skeletons" data-testid="history-skeleton">
        <span class="sr-only">{t('history.loading')}</span>
        <div class="skeleton answer" aria-hidden="true"></div>
        {#each [0, 1, 2] as i (i)}<div class="skeleton tile" aria-hidden="true"></div>{/each}
        {#each [0, 1, 2] as i (i)}<div class="skeleton row" aria-hidden="true"></div>{/each}
      </div>
    {/if}
  {:else if empty === 'unavailable'}
    <section class="card empty" aria-labelledby="history-empty" data-testid="history-empty-unavailable">
      <h2 id="history-empty">{t('history.empty.unavailableTitle')}</h2>
      <p>{t('history.empty.unavailable')}</p>
      {#if overview.unavailableReason}
        <p class="technical"><bdi dir="ltr">{t('history.empty.reason', { reason: overview.unavailableReason })}</bdi></p>
      {/if}
    </section>
  {:else if empty === 'fresh'}
    <section class="card empty" aria-labelledby="history-empty" data-testid="history-empty-fresh">
      <h2 id="history-empty"><ChartLine size={18} aria-hidden="true" />{t('history.empty.freshTitle')}</h2>
      <p>{t('history.empty.fresh')}</p>
    </section>
    {#if logWhenEmpty}{@render events()}{/if}
  {:else if empty === 'range'}
    <section class="card empty" aria-labelledby="history-empty" data-testid="history-empty-range">
      <h2 id="history-empty">{t('history.empty.rangeTitle')}</h2>
      <p>{t('history.empty.range')}</p>
      {#if history.range.kind !== '30d'}
        <button type="button" class="secondary" onclick={() => pickKind('30d')}>
          {t('history.empty.showLast', { n: num(30) })}
        </button>
      {/if}
    </section>
    {#if logWhenEmpty}{@render events()}{/if}
  {:else}
    <HistoryAnswer />
    <HistoryCards />
    <section class="details" aria-labelledby="history-details" data-testid="history-details">
      <h2 id="history-details">{t('history.details.title')}</h2>
      {#each SECTIONS as s (s.id)}
        <DetailsSection id={s.id} title={t(s.title)} icon={ICONS[s.id as keyof typeof ICONS]}>
          {#if s.id === 'signal'}<SignalHistoryCard />
          {:else if s.id === 'speeds'}<SpeedHistoryCard />
          {:else if s.id === 'hours'}<HourBars />
          {:else}<BandSetTable />{/if}
        </DetailsSection>
      {/each}
      {#if ipRanges}
        <DetailsSection id="ipRanges" title={t('history.details.ipRanges')} icon={Network} preview={ipPreview}>
          <IpRangeTable />
        </DetailsSection>
      {/if}
      {@render events()}
      {#if !ipRanges}<p class="muted none" data-testid="ip-none">{t('history.ip.none')}</p>{/if}
    </section>
  {/if}

  {#if overview}
    <footer class="foot">
      {#if overview.available}
        <p class="muted">
          {t('history.status.recordingHint')}
          {overview.retentionDays === null ? t('history.footer.forever') : t('history.footer.kept', { days: num(overview.retentionDays) })}
        </p>
      {/if}
      <div class="foot-actions">
        <button type="button" class="secondary" aria-expanded={showCustom} aria-controls="history-custom-fields" onclick={pickDates}>
          <CalendarDays size={16} aria-hidden="true" />{t('history.range.pick')}
        </button>
        {#if canExport}
          <button
            type="button"
            class="secondary"
            aria-expanded={exportOpen}
            aria-controls="history-export"
            onclick={() => (exportOpen = !exportOpen)}
          >
            <FileDown size={16} aria-hidden="true" />{t('history.export.open')}
          </button>
        {/if}
      </div>
      {#if showCustom}
        <div class="custom" id="history-custom-fields" role="group" aria-label={t('history.range.custom')}>
          <label>
            <span>{t('history.range.from')}</span>
            <input
              id="history-from"
              type="date"
              bind:value={customFrom}
              max={dayValue(today)}
              aria-invalid={!customValid}
              aria-describedby={customValid ? undefined : 'history-range-invalid'}
              onchange={applyCustom}
            />
          </label>
          <label>
            <span>{t('history.range.to')}</span>
            <input
              type="date"
              bind:value={customTo}
              max={dayValue(today)}
              aria-invalid={!customValid}
              aria-describedby={customValid ? undefined : 'history-range-invalid'}
              onchange={applyCustom}
            />
          </label>
          {#if !customValid}<p id="history-range-invalid" class="invalid">{t('history.range.invalid')}</p>{/if}
        </div>
      {/if}
      {#if exportOpen && canExport}
        <div id="history-export"><HistoryExport onclose={() => (exportOpen = false)} /></div>
      {/if}
    </footer>
  {/if}
</section>

{#snippet events()}
  <DetailsSection id="events" title={t('history.details.events')} icon={ScrollText}>
    <EventLog />
  </DetailsSection>
{/snippet}

<style>
  .page {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }
  .titlebar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px 12px;
  }
  h1 {
    margin: 0;
    font-size: var(--fs-title);
    line-height: 32px;
  }
  .lede {
    margin-block: 2px 0;
    color: var(--muted);
    max-inline-size: var(--measure);
  }
  .status {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding-block: 2px;
    padding-inline: 10px;
    border-radius: 999px;
    background: var(--active);
    color: var(--text);
    font-size: var(--fs-small);
    font-weight: 500;
  }
  .status .dot {
    inline-size: 8px;
    block-size: 8px;
    border-radius: 50%;
    background: var(--muted);
  }
  .status.recording {
    background: var(--pill-positive-bg);
    color: var(--pill-positive-ink);
  }
  .status.recording .dot {
    background: currentColor;
  }
  .status.paused,
  .status.loggedOut {
    background: var(--pill-warning-bg);
    color: var(--pill-warning-ink);
  }
  .status.paused .dot,
  .status.loggedOut .dot {
    background: currentColor;
  }
  .ranges {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px 16px;
  }
  .showing {
    color: var(--muted);
    font-size: var(--fs-small);
  }
  .seg {
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
    transition:
      background-color var(--t-fast) var(--ease),
      color var(--t-fast) var(--ease);
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
  .error {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px 12px;
    padding-block: 8px;
    padding-inline: 12px;
    border-radius: var(--radius-control);
    background: var(--pill-negative-bg);
    color: var(--pill-negative-ink);
  }
  .error p {
    margin: 0;
  }
  .card {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius-card);
    padding-block: var(--card-pad-block);
    padding-inline: var(--card-pad-inline);
  }
  .empty {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 8px;
  }
  .empty h2 {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 0;
  }
  .empty p {
    margin: 0;
    max-inline-size: var(--measure);
  }
  .technical {
    color: var(--muted);
    font-size: var(--fs-small);
  }
  .details {
    display: flex;
    flex-direction: column;
    gap: var(--gap);
  }
  .details > h2 {
    margin: 0;
    margin-block-start: 8px;
    font-size: var(--fs-section);
    font-weight: 600;
  }
  .muted {
    margin: 0;
    color: var(--muted);
  }
  .none {
    padding-inline: var(--card-pad-inline);
    font-size: var(--fs-small);
  }
  .foot {
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding-block-start: 12px;
    border-block-start: 1px solid var(--rule);
  }
  .foot p {
    font-size: var(--fs-small);
    max-inline-size: var(--measure);
  }
  .foot-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  .foot-actions button {
    display: inline-flex;
    align-items: center;
    gap: 8px;
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
  .custom {
    display: flex;
    flex-wrap: wrap;
    align-items: flex-end;
    gap: 8px 12px;
  }
  .custom label {
    display: flex;
    flex-direction: column;
    gap: 2px;
    font-size: var(--fs-small);
    color: var(--muted);
  }
  .invalid {
    flex-basis: 100%;
    margin: 0;
    color: var(--pill-negative-ink);
    font-size: var(--fs-small);
  }
  .skeletons {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
    gap: var(--gap);
  }
  .skeleton {
    border-radius: var(--radius-card);
    background: var(--active);
    animation: pulse 1.4s var(--ease) infinite alternate;
  }
  .skeleton.answer {
    grid-column: 1 / -1;
    block-size: 140px;
  }
  .skeleton.tile {
    block-size: 120px;
  }
  .skeleton.row {
    grid-column: 1 / -1;
    block-size: 56px;
  }
  @keyframes pulse {
    from {
      opacity: 1;
    }
    to {
      opacity: 0.5;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .skeleton {
      animation: none;
    }
  }
</style>
