<script lang="ts">
  // Refresh IP progress, on every page while a run is open, and its outcome afterwards. Styled
  // like the safe-apply banner. With the data switch, Stop turns data back on before the run
  // ends; a network search can't be stopped midway, so Stop ends the run after it. After the
  // run it offers what may help next: a band switch, a restart, or the bands from before. None
  // of them starts by itself.
  import RefreshCw from '@lucide/svelte/icons/refresh-cw'
  import CircleCheck from '@lucide/svelte/icons/circle-check'
  import TriangleAlert from '@lucide/svelte/icons/triangle-alert'
  import ArrowLeftRight from '@lucide/svelte/icons/arrow-left-right'
  import Undo2 from '@lucide/svelte/icons/undo-2'
  import X from '@lucide/svelte/icons/x'
  import { api } from '../lib/backend'
  import { app, pushToast } from '../lib/store.svelte'
  import { t, type MessageKey } from '../lib/i18n'
  import { errorText } from '../lib/errors'
  import { formatNumber, isolate } from '../lib/format'
  import { rangeText, targetText } from '../lib/network'
  import { bandsOf, changeOpen, sameConfig } from '../lib/bands'
  import { bandList } from '../lib/eventText'
  import {
    isAutoBands,
    offerReattach,
    offerReboot,
    offerRestoreBands,
    outcomeIsTrouble,
    outcomeKey,
    reattachPlan,
    refusalCode,
  } from '../lib/ipRefresh.svelte'
  import RebootDialog from './RebootDialog.svelte'
  import type { IpRefreshPhase } from '../lib/bindings/IpRefreshPhase'
  import type { NetConfig } from '../lib/bindings/NetConfig'

  const view = $derived(app.snapshot?.ipRefresh ?? null)
  const active = $derived(view?.active === true)
  const last = $derived(view?.last ?? null)
  const digits = $derived(app.settings?.digits ?? 'western')
  const num = (n: number) => isolate(formatNumber(n, { digits }))
  const loggedIn = $derived(app.snapshot?.session.kind === 'loggedIn')
  const search = $derived(view?.method === 'networkSearch')

  const PHASE: Record<IpRefreshPhase, MessageKey> = {
    idle: 'ipRefresh.banner.phase.turningOn',
    turningOff: 'ipRefresh.banner.phase.turningOff',
    off: 'ipRefresh.banner.phase.off',
    turningOn: 'ipRefresh.banner.phase.turningOn',
    confirming: 'ipRefresh.banner.phase.confirming',
    waitingOnline: 'ipRefresh.banner.phase.waitingOnline',
    between: 'ipRefresh.banner.phase.between',
    searching: 'ipRefresh.banner.phase.searching',
  }
  const phaseKey: MessageKey = $derived(
    search && view?.phase === 'waitingOnline'
      ? 'ipRefresh.banner.phase.waitingOnlineSearch'
      : PHASE[view?.phase ?? 'idle'],
  )

  // A result can be dismissed; a new one shows again. Starting a band switch from it dismisses it,
  // and it stays hidden while one runs: its range and offers no longer apply.
  let dismissed: string | null = $state(null)
  const resultKey = $derived(last ? JSON.stringify(last) : null)
  const reattachRunning = $derived(app.snapshot?.ipReattach?.active === true)
  const showResult = $derived(!active && !reattachRunning && last !== null && dismissed !== resultKey)
  const trouble = $derived(last !== null && outcomeIsTrouble(last.outcome))
  const code = $derived(
    last && (last.outcome === 'searchRefused' || last.outcome === 'offRefused') ? refusalCode(last.refusal) : null,
  )

  let stopping = $state(false)
  $effect(() => {
    if (!active) stopping = false
  })

  async function stop() {
    if (stopping || view?.stopping) return
    stopping = true
    try {
      await api.ipRefreshStop()
    } catch (e) {
      stopping = false
      pushToast(errorText(e), 'error')
    }
  }

  const seconds = $derived(view ? (view.secondsLeft ?? view.secondsInPhase) : 0)
  const stopText = $derived(
    stopping || view?.stopping
      ? t(search ? 'ipRefresh.banner.stoppingSearch' : 'ipRefresh.banner.stopping')
      : t(search ? 'ipRefresh.banner.stopSearch' : 'ipRefresh.banner.stop'),
  )

  function bandsText(cfg: NetConfig): string {
    return isAutoBands(cfg) ? t('ipRefresh.reattach.allBands') : bandList(bandsOf(cfg), digits)
  }

  // What may help next.
  const reattach = $derived(showResult && offerReattach(last, app.snapshot))
  const reboot = $derived(showResult && offerReboot(last))
  const restoreTo: NetConfig | null = $derived.by(() => {
    if (!showResult || !offerRestoreBands(last)) return null
    const now = app.snapshot?.config
    // Already back (the user or a later change did it): nothing to offer.
    return now && sameConfig(now, last.bandsBefore) ? null : last.bandsBefore
  })
  const bandsBusy = $derived(changeOpen(app.snapshot?.safeApply) || reattachRunning)

  let restoring = $state(false)
  async function restore() {
    if (!restoreTo || restoring || bandsBusy) return
    restoring = true
    try {
      // A normal Apply: safe-apply verifies it and undoes it if the router loses service.
      await api.applyConfig(restoreTo)
    } catch (e) {
      pushToast(errorText(e), 'error')
    } finally {
      restoring = false
    }
  }

  // The band switch asks once, showing both steps.
  let confirming = $state(false)
  let starting = $state(false)
  let confirmError: string | null = $state(null)
  let cancelBtn: HTMLButtonElement | undefined = $state()
  function openConfirm() {
    confirmError = null
    confirming = true
    queueMicrotask(() => cancelBtn?.focus())
  }
  function closeConfirm() {
    confirming = false
    confirmError = null
  }
  // What the backend would pick now; it picks again when asked and has the final say.
  const plan = $derived(confirming ? reattachPlan(app.snapshot) : null)
  // The backend refuses the switch while automatic undo is off; say so before the click.
  const undoOff = $derived(app.settings?.automation.revertOnLoss === false)
  async function startReattach() {
    if (starting || !plan || undoOff) return
    starting = true
    confirmError = null
    const launchedFrom = resultKey
    try {
      await api.ipReattachStart()
      dismissed = launchedFrom
      closeConfirm()
    } catch (e) {
      confirmError = errorText(e)
    } finally {
      starting = false
    }
  }
</script>

{#if view && active}
  <section
    class="banner"
    class:urgent={view.dataOnFailures > 0}
    role={view.dataOnFailures > 0 ? 'alert' : 'status'}
    aria-label={t('ipRefresh.banner.label')}
    data-testid="ip-refresh-banner"
  >
    <div class="top">
      <p class="title">
        <RefreshCw class="icon" size={16} aria-hidden="true" />
        {#if view.resumed}
          <strong>{t('ipRefresh.banner.resumed')}</strong>
        {:else}
          <strong data-testid="ipr-progress"
            >{t('ipRefresh.banner.tryOf', { n: num(view.tryN), total: num(view.maxTries) })}<span class="sep" aria-hidden="true"
              >·</span
            >{t(phaseKey)}<span class="sep" aria-hidden="true">·</span><span class="num"
              >{t('ipRefresh.banner.seconds', { n: num(seconds) })}</span
            ></strong
          >
        {/if}
      </p>
      <div class="actions">
        <button
          type="button"
          class="secondary"
          disabled={stopping || view.stopping}
          onclick={stop}
          data-testid="ipr-stop">{stopText}</button
        >
      </div>
    </div>
    {#if view.phase === 'searching'}
      <p class="detail" data-testid="ipr-search-note">{t('ipRefresh.banner.searchNote')}</p>
    {/if}
    {#if view.currentRange}
      <p class="detail">
        {t('ipRefresh.banner.range')}
        <bdi dir="ltr" class="num">{rangeText(view.currentRange)}</bdi>
        {#if view.target}<span class="sep" aria-hidden="true">·</span>{t('ipRefresh.banner.aiming')}
          <bdi dir="ltr" class="num">{targetText(view.target)}</bdi>{/if}
      </p>
    {/if}
    {#if view.dataOnFailures > 0}
      <p class="detail strong">{loggedIn ? t('ipRefresh.banner.onFailed') : t('ipRefresh.banner.loginNeeded')}</p>
    {:else if !loggedIn}
      <p class="detail">{t(search ? 'ipRefresh.banner.loginNeededSearch' : 'ipRefresh.banner.loginNeeded')}</p>
    {/if}
  </section>
{:else if showResult && last}
  <section
    class="banner"
    class:done={!trouble}
    class:urgent={trouble}
    role={trouble ? 'alert' : 'status'}
    aria-label={t('ipRefresh.banner.label')}
    data-testid="ip-refresh-banner"
  >
    <div class="top">
      <p class="title">
        {#if trouble}
          <TriangleAlert class="icon" size={16} aria-hidden="true" data-testid="ipr-outcome-trouble" />
        {:else}
          <CircleCheck class="icon" size={16} aria-hidden="true" />
        {/if}
        <strong data-testid="ipr-outcome"
          >{t('ipRefresh.banner.ended', { outcome: t(outcomeKey(last.outcome, last.method)) })}</strong
        >
      </p>
      <div class="actions">
        <button
          type="button"
          class="dismiss"
          aria-label={t('ipRefresh.banner.dismiss')}
          title={t('ipRefresh.banner.dismiss')}
          onclick={() => (dismissed = resultKey)}><X size={14} aria-hidden="true" /></button
        >
      </div>
    </div>
    {#if code !== null}
      <p class="detail" data-testid="ipr-refusal-code">{t('ipRefresh.banner.refusedCode', { code: num(code) })}</p>
    {/if}
    {#if last.to}
      <p class="detail">{t('ipRefresh.banner.nowRange')} <bdi dir="ltr" class="num">{rangeText(last.to)}</bdi></p>
    {/if}
    {#if last.outcome === 'notMatched'}
      <p class="detail" data-testid="ipr-work-note">{t('ipRefresh.dialog.workNote')}</p>
    {/if}
    {#if restoreTo}
      <p class="detail" data-testid="ipr-bands-changed">
        {t('ipRefresh.banner.bandsChanged', { bands: bandsText(restoreTo) })}
      </p>
    {/if}
    {#if reboot}
      <p class="detail">{t('ipRefresh.banner.onlineTimeoutHelp')}</p>
    {/if}
    {#if reattach}
      <p class="detail">{t('ipRefresh.banner.reattachHint')}</p>
    {/if}
    {#if reattach || reboot || restoreTo}
      <div class="offers">
        {#if reattach}
          <button type="button" class="secondary" onclick={openConfirm} data-testid="ipr-reattach"
            ><ArrowLeftRight size={16} aria-hidden="true" />{t('ipRefresh.banner.tryReattach')}</button
          >
        {/if}
        {#if restoreTo}
          <button
            type="button"
            class="secondary"
            disabled={restoring || bandsBusy}
            onclick={restore}
            data-testid="ipr-restore"
            ><Undo2 size={16} aria-hidden="true" />{restoring
              ? t('ipRefresh.banner.restoring')
              : t('ipRefresh.banner.restoreBands')}</button
          >
        {/if}
        {#if reboot}
          <RebootDialog label={t('ipRefresh.banner.restart')} />
        {/if}
      </div>
    {/if}
  </section>
{/if}

{#if confirming}
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div class="backdrop" role="presentation" onkeydown={(e) => e.key === 'Escape' && closeConfirm()}>
    <div
      class="modal"
      role="dialog"
      aria-modal="true"
      aria-labelledby="ipr-reattach-title"
      aria-describedby="ipr-reattach-desc"
    >
      <h2 id="ipr-reattach-title"><ArrowLeftRight size={18} aria-hidden="true" />{t('ipRefresh.reattach.title')}</h2>
      <p id="ipr-reattach-desc">{t('ipRefresh.reattach.body')}</p>
      {#if plan}
        <dl class="steps" data-testid="ipr-reattach-plan">
          <dt>{t('ipRefresh.reattach.temporary')}</dt>
          <dd class="num" data-testid="ipr-reattach-step1">{bandsText(plan.step1)}</dd>
          <dt>{t('ipRefresh.reattach.final')}</dt>
          <dd class="num" data-testid="ipr-reattach-final">{bandsText(plan.final)}</dd>
        </dl>
      {:else}
        <p class="why" role="status">{t('ipRefresh.reattach.unavailable')}</p>
      {/if}
      {#if undoOff}
        <p class="why" role="status" data-testid="ipr-reattach-undo-off">{t('ipRefresh.reattach.undoOff')}</p>
      {/if}
      {#if confirmError}
        <p class="why" role="alert" data-testid="ipr-reattach-error">{confirmError}</p>
      {/if}
      <div class="modal-actions">
        <button type="button" bind:this={cancelBtn} onclick={closeConfirm}>{t('app.cancel')}</button>
        <button
          type="button"
          class="primary"
          disabled={!plan || starting || undoOff}
          onclick={startReattach}
          data-testid="ipr-reattach-start"
          >{starting ? t('ipRefresh.reattach.starting') : t('ipRefresh.reattach.start')}</button
        >
      </div>
    </div>
  </div>
{/if}

<style>
  .banner {
    position: sticky;
    inset-block-start: 4px;
    z-index: 5;
    display: flex;
    flex-direction: column;
    gap: 8px;
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
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 0;
    min-inline-size: 0;
  }
  .title :global(.icon) {
    flex: none;
    color: var(--accent);
  }
  .done .title :global(.icon) {
    color: var(--q-excellent);
  }
  .urgent .title :global(.icon) {
    color: inherit;
  }
  .title strong {
    font-weight: 500;
  }
  .sep {
    margin-inline: 6px;
    color: var(--faint);
  }
  .num {
    font-variant-numeric: tabular-nums;
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
  }
  .actions button.secondary,
  .offers button.secondary {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    min-block-size: var(--control-height);
    padding-block: 0;
    padding-inline: 14px;
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
  .detail {
    margin: 0;
    color: var(--muted);
  }
  .urgent .detail {
    color: inherit;
  }
  .detail.strong {
    font-weight: 500;
  }
  .offers {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
  }
  /* RebootDialog's own row sits in this one. */
  .offers :global(.reboot) {
    margin-block-start: 0;
  }
  /* The band-switch confirm. Below the title bar, so the window can still be moved and closed. */
  .backdrop {
    position: fixed;
    inset: 0;
    inset-block-start: var(--titlebar-height);
    display: grid;
    place-items: center;
    padding: 16px;
    background: var(--scrim);
    z-index: 40;
  }
  .modal {
    background: var(--surface);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: var(--radius-card);
    box-shadow: var(--shadow-pop);
    padding-block: 20px;
    padding-inline: 24px;
    inline-size: min(460px, 100%);
    max-block-size: 100%;
    overflow-y: auto;
    animation: pop-in var(--t-enter) var(--ease);
  }
  .modal h2 {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: var(--fs-section);
    margin-block: 0 8px;
  }
  .modal p {
    margin-block: 0 8px;
  }
  .steps {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 6px 16px;
    margin-block: 4px 12px;
    padding-block: 10px;
    padding-inline: 12px;
    border-radius: var(--radius-control);
    background: var(--raised);
  }
  .steps dt {
    color: var(--muted);
  }
  .steps dd {
    margin: 0;
    font-weight: 500;
  }
  .why {
    color: var(--pill-negative-ink);
    background: var(--pill-negative-bg);
    border-radius: var(--radius-control);
    padding-block: 6px;
    padding-inline: 10px;
  }
  .modal-actions {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: 8px;
    margin-block-start: var(--gap);
  }
</style>
