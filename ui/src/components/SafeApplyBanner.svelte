<script lang="ts">
  import ArrowRight from '@lucide/svelte/icons/arrow-right'
  import X from '@lucide/svelte/icons/x'
  import { api } from '../lib/backend'
  import { app, pushToast } from '../lib/store.svelte'
  import { t, type MessageKey } from '../lib/i18n'
  import { errorText } from '../lib/errors'
  import { formatNumber, isolate } from '../lib/format'
  import { bandsOf, clock, templateParts } from '../lib/bands'
  import { bandList } from '../lib/eventText'
  import type { RevertReason } from '../lib/bindings/RevertReason'
  import type { ApnTarget } from '../lib/bindings/ApnTarget'
  import { setRange } from '../lib/history.svelte'

  // `bands` follows the band change; `apn` the APN switch, which has the same phases. Only one
  // of them can be open at a time.
  let { subject = 'bands' }: { subject?: 'bands' | 'apn' } = $props()
  const isApn = $derived(subject === 'apn')

  const REASON_KEYS: Record<RevertReason, MessageKey> = {
    applyRejected: 'safeApply.reason.rejected',
    verifyMismatch: 'safeApply.reason.verifyMismatch',
    noReattach: 'safeApply.reason.noReattach',
    lossOfService: 'safeApply.reason.connectionLost',
    userRequested: 'safeApply.reason.user',
    resumedWithoutService: 'safeApply.reason.resumedWithoutService',
  }
  /** Used only until settings have loaded; the same as the backend defaults. */
  const DEFAULT_TIMERS = { graceS: 90, lossS: 60, probationS: 180 }
  /** An APN switch's timers are fixed in the backend (`APN_TIMERS`). */
  const APN_TIMERS = { graceS: 90, lossS: 60, probationS: 120 }
  /** Texts that name bands, in their APN wording. */
  const APN_TEXT = {
    title: 'safeApply.apn.title',
    applying: 'safeApply.apn.applying',
    reverting: 'safeApply.apn.reverting',
    revertPending: 'safeApply.apn.revertPending',
    committed: 'safeApply.apn.committed',
    reverted: 'safeApply.apn.reverted',
    revertFailedTitle: 'safeApply.apn.revertFailedTitle',
    revertFailedHelp: 'safeApply.apn.revertFailedHelp',
    closeWebUi: 'safeApply.apn.closeWebUi',
  } as const satisfies Record<string, MessageKey>
  const BAND_TEXT = {
    title: 'safeApply.title',
    applying: 'safeApply.applying',
    reverting: 'safeApply.reverting',
    revertPending: 'safeApply.revertPending',
    committed: 'safeApply.committed',
    reverted: 'safeApply.reverted',
    revertFailedTitle: 'safeApply.revertFailedTitle',
    revertFailedHelp: 'safeApply.revertFailedHelp',
    closeWebUi: 'safeApply.closeWebUi',
  } as const satisfies Record<keyof typeof APN_TEXT, MessageKey>
  const K = $derived(isApn ? APN_TEXT : BAND_TEXT)

  const bandView = $derived(isApn ? null : (app.snapshot?.safeApply ?? null))
  const apnView = $derived(isApn ? (app.snapshot?.apnApply ?? null) : null)
  const view = $derived(bandView ?? apnView)
  const phase = $derived(view?.phase ?? { kind: 'idle' as const })
  const digits = $derived(app.settings?.digits ?? 'western')
  const takenOver = $derived(app.snapshot?.session.kind === 'takenOver')
  const timers = $derived(isApn ? APN_TIMERS : (app.settings?.timers ?? DEFAULT_TIMERS))
  const idPrefix = $derived(isApn ? 'apn-' : '')

  // Two-step apply (primary band first) and the "auto-undo off" policy, from the backend.
  const sequence = $derived(bandView?.sequence ?? null)
  // Step 1 is kept but the other bands aren't back yet: the change is still open.
  const between = $derived(phase.kind === 'committed' && sequence !== null)
  // A band switch for a new IP: step 1 is its own band set, step 2 the bands from before.
  const reattach = $derived(sequence?.origin === 'ipReattach')
  const finalBandsText = $derived(sequence ? bandList(bandsOf(sequence.finalConfig), digits) : '')

  // Terminal states can be dismissed; the key changes whenever the phase does.
  let dismissedKey: string | null = $state(null)
  const phaseKey = $derived(
    phase.kind === 'reverted' ? `reverted:${phase.reason}:${phase.fallback}` : phase.kind,
  )
  const terminal = $derived(
    (phase.kind === 'committed' && !between) || phase.kind === 'reverted' || phase.kind === 'revertFailed',
  )
  // A later identical outcome must show again, so forget the dismissal once the phase leaves a terminal state.
  $effect(() => {
    if (!terminal) dismissedKey = null
  })
  /** Opens the newest band change in the History page's event log. Nothing about the change waits on it. */
  function seeWhatHappened() {
    app.historyFocus = 'latestChange'
    void setRange({ kind: '24h' })
    app.tab = 'history'
  }

  const visible = $derived(phase.kind !== 'idle' && !(terminal && dismissedKey === phaseKey))

  const previousBands = $derived(bandsOf(bandView?.previous))
  const requestedBands = $derived(bandsOf(bandView?.requested))

  /** A profile's name from the last read, or its number when it isn't in the list. */
  function apnName(target: ApnTarget | null | undefined): string | null {
    if (!target) return null
    const p = app.snapshot?.apn?.profiles.find((x) => x.index === target.index)
    return p?.name || t('safeApply.apn.profileNumber', { n: formatNumber(target.index, { digits }) })
  }
  const previousApn = $derived(apnName(apnView?.previous))
  const requestedApn = $derived(apnName(apnView?.requested))

  const text = $derived.by(() => {
    switch (phase.kind) {
      case 'idle': return ''
      case 'snapshotting': return t('safeApply.snapshotting')
      case 'applying': return t(K.applying)
      case 'verifying': return t('safeApply.verifying')
      case 'reattachGrace': return t('safeApply.reattachGrace')
      case 'probation': return t('safeApply.probation')
      case 'reverting':
        return phase.fallback
          ? t('safeApply.revertingAuto')
          : // `attempt` counts failed attempts, so the one in flight is attempt + 1.
            t(K.reverting, { attempt: isolate(formatNumber(phase.attempt + 1, { digits })) })
      case 'revertPending': return t(K.revertPending)
      // "Kept {bands}." when the requested bands are known; the slot is filled in the markup.
      case 'committed':
        if (sequence && reattach) return t('safeApply.reattachWaiting', { bands: finalBandsText })
        if (sequence) return t('safeApply.sequenceWaiting', { band: isolate('B' + formatNumber(sequence.primary, { digits })) })
        return requestedBands.length ? t('safeApply.committedBands') : t(K.committed)
      case 'reverted': return phase.fallback ? t('safeApply.revertedToAuto') : t(K.reverted)
      case 'revertFailed': return t(K.revertFailedTitle)
    }
  })
  const reason = $derived(
    phase.kind === 'reverted' || phase.kind === 'revertPending' ? t(REASON_KEYS[phase.reason]) : null,
  )

  /** "Operator internet → Work" while an APN switch runs, the restored profile after a revert. */
  const apnRoute = $derived.by((): { from: string; to: string | null } | null => {
    if (!isApn || !previousApn) return null
    switch (phase.kind) {
      case 'snapshotting':
      case 'applying':
      case 'verifying':
      case 'reattachGrace':
      case 'probation':
        return requestedApn ? { from: previousApn, to: requestedApn } : null
      case 'reverting':
      case 'revertPending':
      case 'reverted':
        return { from: previousApn, to: null }
      default:
        return null
    }
  })

  /** "B1, B3 → B3" while the change runs, the restored bands after a revert. */
  const route = $derived.by((): { from: number[]; to: number[] | null } | null => {
    switch (phase.kind) {
      case 'snapshotting':
      case 'applying':
      case 'verifying':
      case 'reattachGrace':
      case 'probation':
        return previousBands.length && requestedBands.length ? { from: previousBands, to: requestedBands } : null
      case 'reverting':
      case 'revertPending':
        return previousBands.length && !(phase.kind === 'reverting' && phase.fallback)
          ? { from: previousBands, to: null }
          : null
      case 'reverted':
        return previousBands.length && !phase.fallback ? { from: previousBands, to: null } : null
      default:
        return null
    }
  })

  const showSeconds = $derived(
    view?.secondsLeft != null && (phase.kind === 'reattachGrace' || phase.kind === 'probation'),
  )
  // "m:ss" in the user's digit style.
  const countdown = $derived(
    clock(view?.secondsLeft ?? 0).replace(/[0-9]/g, (d) => formatNumber(Number(d), { digits })),
  )
  const showKeep = $derived(phase.kind === 'reattachGrace' || phase.kind === 'probation')
  const canKeep = $derived(phase.kind === 'probation')
  const showRevert = $derived(
    ['snapshotting', 'applying', 'verifying', 'reattachGrace', 'probation', 'revertPending', 'revertFailed'].includes(
      phase.kind,
    ),
  )
  // The backend ignores RevertNow before the change is applied and verified; after a failed
  // revert it starts the whole revert again.
  const canRevert = $derived(
    phase.kind === 'reattachGrace' ||
      phase.kind === 'probation' ||
      phase.kind === 'revertPending' ||
      phase.kind === 'revertFailed',
  )
  const urgent = $derived(
    phase.kind === 'revertFailed' || (phase.kind === 'revertPending' && takenOver),
  )

  // The six-step track. `current` is the step in progress; steps before it are done.
  const STEPS = ['saved', 'applied', 'checked', 'reconnect', 'trial', 'kept'] as const
  const current = $derived.by((): number | null => {
    switch (phase.kind) {
      case 'snapshotting': return 0
      case 'applying': return 1
      case 'verifying': return 2
      case 'reattachGrace': return 3
      case 'probation': return 4
      case 'committed': return STEPS.length
      default: return null
    }
  })
  const stepLabel = (s: (typeof STEPS)[number], i: number): string => {
    if (s !== 'reconnect') return t(`safeApply.step.${s}`)
    return current !== null && i < current ? t('safeApply.step.reconnected') : t('safeApply.step.reconnecting')
  }
  // Timed steps show how much of their window has passed; the others are indeterminate.
  const progress = $derived.by((): number | null => {
    const left = view?.secondsLeft
    if (left == null) return null
    const total = phase.kind === 'reattachGrace' ? timers.graceS : phase.kind === 'probation' ? timers.probationS : 0
    if (total <= 0) return null
    return Math.min(1, Math.max(0, 1 - left / total))
  })

  /** How long a Keep/Revert click may go unanswered before the buttons come back. */
  const NO_RESPONSE_MS = 15_000

  // Keep/Revert only queue a command; the backend answers by changing the phase. Until it
  // does, both buttons are disabled and the clicked one says what is happening.
  let pending: 'keep' | 'revert' | null = $state(null)
  let stuck: ReturnType<typeof setTimeout> | undefined
  $effect(() => {
    void phaseKey
    pending = null
    clearTimeout(stuck)
  })

  async function run(kind: 'keep' | 'revert', f: () => Promise<void>) {
    if (pending) return
    pending = kind
    clearTimeout(stuck)
    stuck = setTimeout(() => {
      pending = null
      pushToast(t('safeApply.noResponse'), 'error')
    }, NO_RESPONSE_MS)
    try {
      await f()
    } catch (e) {
      pending = null
      clearTimeout(stuck)
      pushToast(errorText(e), 'error')
    }
  }

  const sequenceText = $derived.by(() => {
    if (!sequence || phase.kind === 'idle' || phase.kind === 'revertFailed') return null
    if (reattach) {
      return sequence.step === 1
        ? t('safeApply.reattachStep1', { bands: bandList(bandsOf(sequence.step1 ?? null), digits) })
        : t('safeApply.reattachStep2', { bands: finalBandsText })
    }
    const band = isolate('B' + formatNumber(sequence.primary, { digits }))
    return sequence.step === 1 ? t('safeApply.sequenceStep1', { band }) : t('safeApply.sequenceStep2', { band })
  })
  const lossIgnored = $derived(bandView?.policy?.lossIgnored === true && !terminal)
</script>

{#snippet bandNames(list: number[])}
  {#each list as b, i (b)}{#if i > 0}{t('bands.listSep')}{/if}<bdi dir="ltr">B{b}</bdi>{/each}
{/snippet}

{#if visible}
  <section
    class="banner"
    class:urgent
    class:done={phase.kind === 'committed' && !between}
    role={urgent ? 'alert' : 'status'}
    aria-label={t(K.title)}
    data-subject={subject}
  >
    <div class="top">
      <p class="title">
        <strong class="text"
          >{#if phase.kind === 'committed' && !between && requestedBands.length}{#each templateParts(text) as part, i (i)}{#if 'text' in part}{part.text}{:else}{@render bandNames(
                  requestedBands,
                )}{/if}{/each}{:else}{text}{/if}</strong
        >{#if apnRoute}<span class="route" data-testid="apn-route"
            ><span class="sep" aria-hidden="true">·</span>
            <bdi>{apnRoute.from}</bdi>{#if apnRoute.to}<ArrowRight class="arrow" size={14} aria-hidden="true" /><span
                class="sr-only">{t('safeApply.routeTo')}</span
              ><bdi>{apnRoute.to}</bdi>{/if}</span
          >{/if}{#if route}<span class="route"
            ><span class="sep" aria-hidden="true">·</span>
            {@render bandNames(route.from)}{#if route.to}<ArrowRight class="arrow" size={14} aria-hidden="true" /><span class="sr-only"
                >{t('safeApply.routeTo')}</span
              >{@render bandNames(route.to)}{/if}</span
          >{/if}
      </p>
      {#if sequenceText}<p class="note num">{sequenceText}</p>{/if}
      {#if lossIgnored}<p class="note loss" role="alert">{t('safeApply.lossIgnored')}</p>{/if}

      {#if showSeconds}
        <span class="clock num"
          >{#each templateParts(t('safeApply.timeLeft')) as part, i (i)}{#if 'text' in part}<span class="unit"
                >{part.text}</span
              >{:else}<bdi dir="ltr">{countdown}</bdi>{/if}{/each}</span
        >
      {/if}

      <div class="actions">
        {#if showKeep}
          <button
            type="button"
            class="primary"
            disabled={!canKeep || pending !== null}
            title={canKeep ? undefined : t('safeApply.keepUnavailable')}
            aria-describedby={canKeep ? undefined : `${idPrefix}keep-reason`}
            onclick={() => run('keep', api.keepNow)}
            >{pending === 'keep' ? t('safeApply.keeping') : t('safeApply.keep')}</button
          >
          {#if !canKeep}<span id="{idPrefix}keep-reason" class="sr-only">{t('safeApply.keepUnavailable')}</span>{/if}
        {/if}
        {#if showRevert}
          <button
            type="button"
            class={phase.kind === 'revertFailed' ? 'primary' : 'secondary'}
            disabled={!canRevert || pending !== null}
            aria-describedby={canRevert ? undefined : `${idPrefix}revert-reason`}
            onclick={() => run('revert', api.revertNow)}
            >{pending === 'revert' ? t('safeApply.revertingNow') : t('safeApply.revert')}</button
          >
        {/if}
        {#if terminal}
          <button
            type="button"
            class="dismiss"
            aria-label={t('safeApply.dismiss')}
            title={t('safeApply.dismiss')}
            onclick={() => (dismissedKey = phaseKey)}><X size={14} aria-hidden="true" /></button
          >
        {/if}
      </div>
    </div>

    {#if showRevert && !canRevert}<p id="{idPrefix}revert-reason" class="hint">{t('safeApply.revertUnavailable')}</p>{/if}

    {#if phase.kind === 'reverted' && reason}
      <p class="detail"><span class="pill">{t('safeApply.revertedPill')}</span><span>{reason}</span></p>
    {:else if reason}
      <p class="detail"><span>{reason}</span></p>
    {/if}
    {#if phase.kind === 'revertPending' && takenOver}
      <p class="close-web">{t(K.closeWebUi)}</p>
    {/if}
    {#if phase.kind === 'revertFailed'}
      <p class="detail">
        {t(K.revertFailedHelp, { host: isolate(app.settings?.host ?? '192.168.8.1') })}
      </p>
    {/if}
    {#if !isApn && (phase.kind === 'reverted' || phase.kind === 'revertFailed')}
      <p class="detail"><button type="button" class="link" onclick={seeWhatHappened}>{t('safeApply.seeWhatHappened')}</button></p>
    {/if}

    {#if current !== null}
      <ol class="track" aria-label={t('safeApply.trackLabel')}>
        {#each STEPS as s, i (s)}
          <li
            class:done={i < current}
            class:now={i === current}
            aria-current={i === current ? 'step' : undefined}
          >
            <span class="bar"
              >{#if i === current}<span
                  class="fill"
                  class:indeterminate={progress === null}
                  style:inline-size={progress === null ? undefined : `${progress * 100}%`}
                ></span>{/if}</span
            >
            <span class="label">{stepLabel(s, i)}</span>
          </li>
        {/each}
      </ol>
    {/if}
  </section>
{/if}

<style>
  /* Keep and Revert stay on screen however far the page below is scrolled. */
  .banner {
    position: sticky;
    inset-block-start: 4px;
    z-index: 5;
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding-block: 14px;
    padding-inline: 18px;
    border: 1px solid var(--border);
    border-radius: var(--radius-card);
    background: var(--surface);
    color: var(--text);
    box-shadow: var(--shadow-pop);
    animation: pop-in var(--t-enter) var(--ease);
  }
  .banner.done {
    background: color-mix(in srgb, var(--q-excellent) 8%, var(--surface));
    border-color: color-mix(in srgb, var(--q-excellent) 40%, var(--border));
  }
  .banner.urgent {
    background: var(--pill-negative-bg);
    border-color: color-mix(in srgb, var(--q-poor) 35%, transparent);
    color: var(--pill-negative-ink);
  }
  .top {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px 12px;
  }
  .title {
    flex: 1 1 260px;
    margin: 0;
    min-inline-size: 0;
  }
  .text {
    font-weight: 500;
  }
  .route {
    color: var(--faint);
    white-space: nowrap;
  }
  .sep {
    margin-inline: 6px;
  }
  .route :global(.arrow) {
    margin-inline: 4px;
    vertical-align: -2px;
  }
  /* The arrow points the reading direction. */
  :global([dir='rtl']) .route :global(.arrow) {
    transform: scaleX(-1);
  }
  .clock {
    font-size: var(--fs-value);
    white-space: nowrap;
  }
  .clock .unit {
    color: var(--faint);
    font-size: var(--fs-small);
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
  }
  .actions button {
    block-size: var(--control-height);
    padding-block: 0;
    padding-inline: 14px;
  }
  .actions button.secondary {
    background: var(--secondary);
    border-color: var(--secondary);
    font-weight: 500;
  }
  .actions button.dismiss {
    display: grid;
    place-items: center;
    inline-size: 28px;
    block-size: 28px;
    padding: 0;
    border: none;
    background: transparent;
    color: var(--muted);
  }
  .actions button.dismiss:hover {
    background: var(--active);
    color: var(--text);
  }
  .urgent .actions button.dismiss {
    color: inherit;
  }
  .hint {
    margin: 0;
    color: var(--muted);
    font-size: var(--fs-small);
  }
  .detail {
    margin: 0;
    color: var(--muted);
  }
  .urgent .detail {
    color: inherit;
  }
  button.link {
    all: unset;
    color: var(--text);
    text-decoration: underline;
    text-underline-offset: 2px;
    cursor: pointer;
    font-size: var(--fs-small);
  }
  button.link:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  .pill {
    display: inline-flex;
    align-items: center;
    block-size: 20px;
    margin-inline-end: 8px;
    padding-inline: 8px;
    border-radius: var(--radius-chip);
    background: var(--pill-warning-bg);
    color: var(--pill-warning-ink);
    font-size: var(--fs-small);
    font-weight: 500;
  }
  .close-web {
    margin: 0;
    font-weight: 600;
  }
  .track {
    display: grid;
    grid-template-columns: repeat(6, minmax(0, 1fr));
    gap: 4px;
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .track li {
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-inline-size: 0;
    color: var(--faint);
    font-size: var(--fs-small);
  }
  .track .label {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .track li.done {
    color: var(--muted);
  }
  .track li.now {
    color: var(--text);
    font-weight: 500;
  }
  .bar {
    position: relative;
    display: block;
    block-size: 4px;
    overflow: hidden;
    border-radius: 2px;
    background: var(--q-track);
  }
  .track li.done .bar {
    background: var(--primary);
  }
  .track li.now .bar {
    background: color-mix(in srgb, var(--accent) 22%, var(--q-track));
  }
  .fill {
    position: absolute;
    inset-block: 0;
    inset-inline-start: 0;
    border-radius: 2px;
    background: var(--accent);
    transition: inline-size 1s linear;
  }
  .fill.indeterminate {
    inline-size: 35%;
    animation: slide 1.4s ease-in-out infinite;
  }
  @keyframes slide {
    from {
      inset-inline-start: -35%;
    }
    to {
      inset-inline-start: 100%;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .fill.indeterminate {
      animation: none;
      inline-size: 50%;
    }
  }
  .sr-only {
    position: absolute;
    inline-size: 1px;
    block-size: 1px;
    overflow: hidden;
    clip-path: inset(50%);
    white-space: nowrap;
  }
  .note {
    margin: 0;
    color: var(--muted);
    font-size: var(--fs-small);
    flex-basis: 100%;
  }
  .note.loss {
    color: var(--pill-negative-ink);
    background: var(--pill-negative-bg);
    border-radius: var(--radius-control);
    padding-block: 6px;
    padding-inline: 10px;
    font-size: var(--fs-body);
  }
</style>
