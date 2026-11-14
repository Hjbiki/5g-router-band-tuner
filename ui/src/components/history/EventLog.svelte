<script lang="ts">
  // The History page's event log: what the app did and saw in the range, by day, newest first.
  // A band change is one card with its steps folded away. Severity is always a word, never colour
  // alone. It lives in the History page's "What happened" section, which only mounts it while open,
  // so it loads when the section opens.
  import Layers from '@lucide/svelte/icons/layers'
  import Wifi from '@lucide/svelte/icons/wifi'
  import Gauge from '@lucide/svelte/icons/gauge'
  import Router from '@lucide/svelte/icons/router'
  import AppWindow from '@lucide/svelte/icons/app-window'
  import { tick, untrack } from 'svelte'
  import { app } from '../../lib/store.svelte'
  import { t, currentLanguage } from '../../lib/i18n'
  import { history } from '../../lib/history.svelte'
  import { EVENT_FILTERS, eventLog, eventsKey, loadEvents, type EventFilter } from '../../lib/events.svelte'
  import { changeSeverity, changeTitle, renderEventText, translate } from '../../lib/eventText'
  import type { EventCategory } from '../../lib/bindings/EventCategory'
  import type { EventEntry } from '../../lib/bindings/EventEntry'
  import type { EventItem } from '../../lib/bindings/EventItem'
  import type { Severity } from '../../lib/bindings/Severity'
  import { formatDay, formatTime } from './TimeChart.svelte'

  const digits = $derived(app.settings?.digits ?? 'western')
  const lang = $derived(currentLanguage())
  const numbering = $derived(digits === 'arabicIndic' ? 'arab' : 'latn')
  const clock = (ms: number) => formatTime(ms, lang, digits)
  const clockS = (ms: number) =>
    new Intl.DateTimeFormat(lang, { hour: '2-digit', minute: '2-digit', second: '2-digit', hourCycle: 'h23', numberingSystem: numbering }).format(ms)

  const ICONS: Record<EventCategory, typeof Layers> = {
    bandChange: Layers,
    connection: Wifi,
    speedTest: Gauge,
    device: Router,
    app: AppWindow,
  }

  // Band changes whose steps are shown, by start time.
  let expanded: number[] = $state([])
  const toggleSteps = (ms: number) => (expanded = expanded.includes(ms) ? expanded.filter((x) => x !== ms) : [...expanded, ms])

  /** Loads the first page unless the log already shows it. */
  function ensureLoaded(): Promise<boolean> {
    const range = history.loadedRange
    if (!range) return Promise.resolve(false)
    if (eventLog.loadedKey === eventsKey(range) && !eventLog.error) return Promise.resolve(true)
    return loadEvents(range)
  }

  function setFilter(f: EventFilter) {
    eventLog.filter = f
    void ensureLoaded()
  }
  function setFlag(name: 'problemsOnly' | 'technical', value: boolean) {
    eventLog[name] = value
    void ensureLoaded()
  }

  // Shown, and each new range on the page: the log follows it.
  $effect(() => {
    if (!history.loadedRange) return
    untrack(() => {
      if (app.historyFocus !== 'latestChange') void ensureLoaded()
    })
  })

  // The Bands page's banner sent the user here: open the log on band changes, unfold the newest
  // change and put the keyboard focus on its heading, once. It waits for the range the banner
  // asked for: the one kept from an earlier visit ends before the change.
  $effect(() => {
    if (app.historyFocus !== 'latestChange' || !history.loadedRange || history.loading) return
    app.historyFocus = null
    untrack(() => {
      eventLog.filter = 'bandChange'
      eventLog.problemsOnly = false
      void showLatestChange()
    })
  })

  async function showLatestChange() {
    if (!(await ensureLoaded())) return
    const first = eventLog.entries.find((e) => e.kind === 'change')
    if (!first || first.kind !== 'change') return
    const ms = first.group.startedMs
    if (!expanded.includes(ms)) expanded = [...expanded, ms]
    await tick()
    const h = document.getElementById(`ev-change-${ms}`)
    h?.scrollIntoView?.({ behavior: 'smooth', block: 'start' })
    h?.focus({ preventScroll: true })
  }

  const at = (e: EventEntry) => (e.kind === 'change' ? e.group.startedMs : e.item.atMs)

  function dayLabel(ms: number): string {
    const day = (x: number) => {
      const d = new Date(x)
      return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime()
    }
    const diff = Math.round((day(Date.now()) - day(ms)) / 86_400_000)
    if (diff === 0) return t('events.today')
    if (diff === 1) return t('events.yesterday')
    return formatDay(ms, lang, digits)
  }

  // Entries by local day, in the order the backend sent them (newest first).
  const days = $derived.by(() => {
    const out: { label: string; key: string; entries: EventEntry[] }[] = []
    for (const e of eventLog.entries) {
      const label = dayLabel(at(e))
      const last = out[out.length - 1]
      if (last && last.label === label) last.entries.push(e)
      else out.push({ label, key: `${label}-${at(e)}`, entries: [e] })
    }
    return out
  })

  const sentence = (i: EventItem) => renderEventText(i.key, i.params, digits)
  const shownSteps = (steps: EventItem[]) => (eventLog.technical ? steps : steps.filter((s) => !s.technical))
  const pill = (s: Severity) => t(`events.severity.${s}`)
</script>

<div class="body" data-testid="event-log">
  <p class="muted small">{t('events.intro')}</p>
  <div class="filters">
    <div class="seg" role="radiogroup" aria-label={t('events.filterLabel')}>
      {#each EVENT_FILTERS as f (f)}
        <label class:on={eventLog.filter === f}>
          <input type="radio" name="ev-filter" checked={eventLog.filter === f} onchange={() => setFilter(f)} />
          {t(`events.filter.${f}`)}
        </label>
      {/each}
    </div>
    <label class="check">
      <input type="checkbox" checked={eventLog.problemsOnly} onchange={(e) => setFlag('problemsOnly', e.currentTarget.checked)} />
      {t('events.problemsOnly')}
    </label>
    <label class="check">
      <input type="checkbox" checked={eventLog.technical} onchange={(e) => setFlag('technical', e.currentTarget.checked)} />
      {t('events.technical')}
    </label>
  </div>

  {#if eventLog.error}
    <p class="error" role="alert">{t('events.error', { message: eventLog.error })}</p>
  {/if}

  {#if eventLog.entries.length === 0}
    <p class="muted" aria-live="polite">{eventLog.loading ? t('events.loading') : t('events.empty')}</p>
  {:else}
    <div class="days" aria-busy={eventLog.loading}>
      {#each days as d (d.key)}
        <section class="day" aria-label={d.label}>
          <h4 class="dayhead">{d.label}</h4>
          <ul>
            {#each d.entries as e, idx (idx)}
              {#if e.kind === 'change'}
                {@const g = e.group}
                {@const sev = changeSeverity(g.outcome)}
                {@const steps = shownSteps(g.steps)}
                <li class="change" data-testid="change">
                  <span class="time num">{clock(g.startedMs)}</span>
                  <Layers size={16} aria-hidden="true" />
                  <div class="what">
                    <h5 id="ev-change-{g.startedMs}" tabindex="-1">{changeTitle(g, digits)}</h5>
                    {#if g.outcome.kind === 'reverted'}
                      <p class="why">{t('events.whyLine', { reason: translate(g.outcome.reason) })}</p>
                      {#if g.outcome.fallback}<p class="why">{t('events.why.fellBackToAuto')}</p>{/if}
                    {:else if g.outcome.kind === 'revertFailed'}
                      <p class="why">{t('events.why.revertFailed')}</p>
                    {/if}
                    {#if steps.length}
                      <button
                        type="button"
                        class="link"
                        aria-expanded={expanded.includes(g.startedMs)}
                        aria-controls="ev-steps-{g.startedMs}"
                        onclick={() => toggleSteps(g.startedMs)}>{t('events.steps')}</button
                      >
                      {#if expanded.includes(g.startedMs)}
                        <ol class="steps" id="ev-steps-{g.startedMs}" data-testid="steps">
                          {#each steps as s, i (i)}
                            <li><span class="time num">{clockS(s.atMs)}</span> <span>{sentence(s)}</span></li>
                          {/each}
                        </ol>
                      {/if}
                    {/if}
                  </div>
                  <span class="pill sev-{sev}">{pill(sev)}</span>
                </li>
              {:else}
                {@const i = e.item}
                {@const Icon = ICONS[i.category] ?? AppWindow}
                <li class="single" class:technical={i.technical}>
                  <span class="time num">{clock(i.atMs)}</span>
                  <Icon size={16} role="img" aria-label={t(`events.category.${i.category}`)} />
                  <div class="what">
                    <p>{sentence(i)}</p>
                    {#if i.detailKey}<p class="why">{translate(i.detailKey)}</p>{/if}
                  </div>
                  <span class="pill sev-{i.severity}">{pill(i.severity)}</span>
                </li>
              {/if}
            {/each}
          </ul>
        </section>
      {/each}
    </div>
    {#if eventLog.nextBeforeMs !== null}
      <button
        type="button"
        class="secondary older"
        disabled={eventLog.loading || !history.loadedRange}
        onclick={() => history.loadedRange && void loadEvents(history.loadedRange, true)}>{t('events.older')}</button
      >
    {/if}
  {/if}
</div>

<style>
  .body {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .filters {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px 16px;
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
  .check {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: var(--fs-small);
  }
  .days {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .dayhead {
    margin: 0;
    padding-block-end: 4px;
    border-block-end: 1px solid var(--rule);
    font-size: var(--fs-small);
    font-weight: 600;
    color: var(--muted);
  }
  ul {
    margin: 0;
    padding: 0;
    list-style: none;
  }
  li.single,
  li.change {
    display: grid;
    grid-template-columns: auto auto minmax(0, 1fr) auto;
    align-items: start;
    gap: 4px 10px;
    padding-block: 8px;
    border-block-end: 1px solid var(--rule);
  }
  li.change {
    padding-inline: 8px;
    border-radius: var(--radius-control);
    background: var(--raised);
    border-block-end: 0;
    margin-block: 4px;
  }
  li :global(svg) {
    margin-block-start: 2px;
    color: var(--muted);
  }
  .time {
    color: var(--muted);
    font-size: var(--fs-small);
    font-variant-numeric: tabular-nums;
    line-height: 20px;
  }
  .what p,
  .what h5 {
    margin: 0;
  }
  .what h5 {
    font-size: inherit;
    font-weight: 600;
  }
  .what h5:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  .why {
    color: var(--muted);
    font-size: var(--fs-small);
  }
  .steps {
    display: flex;
    flex-direction: column;
    gap: 2px;
    margin-block: 6px 0;
    padding-inline-start: 0;
    list-style: none;
    font-size: var(--fs-small);
  }
  .pill {
    padding-block: 1px;
    padding-inline: 8px;
    border-radius: 999px;
    font-size: var(--fs-small);
    white-space: nowrap;
    background: var(--active);
    color: var(--text);
  }
  .pill.sev-good {
    background: var(--pill-positive-bg);
    color: var(--pill-positive-ink);
  }
  .pill.sev-warning {
    background: var(--pill-warning-bg);
    color: var(--pill-warning-ink);
  }
  .pill.sev-problem {
    background: var(--pill-negative-bg);
    color: var(--pill-negative-ink);
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
  button.older {
    align-self: center;
    min-block-size: var(--control-height);
    padding-inline: 14px;
    background: var(--secondary);
    border-color: var(--secondary);
    font-weight: 500;
  }
  .error {
    margin: 0;
    padding-block: 8px;
    padding-inline: 12px;
    border-radius: var(--radius-control);
    background: var(--pill-negative-bg);
    color: var(--pill-negative-ink);
  }
  .muted {
    margin: 0;
    color: var(--muted);
  }
  .small {
    font-size: var(--fs-small);
  }
</style>
