<script lang="ts">
  // Refresh IP: the one place a run starts. Mounted once in App and opened through
  // `openIpRefresh`, from the Network page or from History with an address start filled in.
  import { untrack } from 'svelte'
  import RefreshCw from '@lucide/svelte/icons/refresh-cw'
  import Minus from '@lucide/svelte/icons/minus'
  import Plus from '@lucide/svelte/icons/plus'
  import { api } from '../lib/backend'
  import { app } from '../lib/store.svelte'
  import { t } from '../lib/i18n'
  import { errorText } from '../lib/errors'
  import { formatNumber, isolate } from '../lib/format'
  import { scanActive } from '../lib/scan.svelte'
  import { speed } from '../lib/speedtest.svelte'
  import {
    MAX_TRIES,
    closeIpRefresh,
    estimateSeconds,
    ipRefreshUi,
    normalizeTarget,
    preferredMethod,
    startRefusal,
  } from '../lib/ipRefresh.svelte'

  const digits = $derived(app.settings?.digits ?? 'western')
  const num = (n: number) => isolate(formatNumber(n, { digits }))

  let aim = $state(false)
  let target = $state('')
  let tries = $state(MAX_TRIES)
  let busy = $state(false)
  let error: string | null = $state(null)
  let cancelBtn: HTMLButtonElement | undefined = $state()

  // Each opening starts from the History prefill, or from what the last run used.
  $effect(() => {
    if (!ipRefreshUi.open) return
    untrack(() => {
      const saved = app.settings?.ipRefresh
      const prefill = ipRefreshUi.prefill
      aim = prefill !== null
      target = prefill ?? saved?.lastTarget ?? ''
      tries = Math.min(MAX_TRIES, Math.max(1, saved?.maxTries ?? MAX_TRIES))
      error = null
      busy = false
    })
    queueMicrotask(() => cancelBtn?.focus())
  })

  const normalized = $derived(normalizeTarget(target))
  // What the rest of the address may be: "10" leaves three numbers open, "10.45" two.
  const tail = $derived((normalized ?? target).split('.').length >= 2 ? '.x.x' : '.x.x.x')
  const targetInvalid = $derived(aim && normalized === null)
  const refusal = $derived(startRefusal(app.snapshot, { scan: scanActive(), speedTest: speed.running }))
  const lastNotMatched = $derived(app.snapshot?.ipRefresh?.last?.outcome === 'notMatched')
  const canStart = $derived(!busy && refusal === null && !targetInvalid)

  // The router's first method; the data-switch wording stays for drivers that use it.
  const method = $derived(preferredMethod(app.snapshot) ?? 'dataSwitch')
  const search = $derived(method === 'networkSearch')

  const estimate = $derived.by(() => {
    const s = estimateSeconds(aim ? tries : 1, method)
    return s < 90
      ? t('ipRefresh.dialog.aboutSeconds', { n: num(s) })
      : t('ipRefresh.dialog.aboutMinutes', { n: num(Math.ceil(s / 60)) })
  })

  async function start() {
    if (!canStart) return
    busy = true
    error = null
    try {
      await api.ipRefreshStart(aim ? normalized : null, aim ? tries : 1)
      closeIpRefresh()
    } catch (e) {
      error = errorText(e)
    } finally {
      busy = false
    }
  }

  const step = (d: number) => (tries = Math.min(MAX_TRIES, Math.max(1, tries + d)))
</script>

{#if ipRefreshUi.open}
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div class="backdrop" role="presentation" onkeydown={(e) => e.key === 'Escape' && closeIpRefresh()}>
    <div class="modal" role="dialog" aria-modal="true" aria-labelledby="ipr-title" aria-describedby="ipr-desc">
      <h2 id="ipr-title"><RefreshCw size={18} aria-hidden="true" />{t('ipRefresh.dialog.title')}</h2>
      <p id="ipr-desc" data-testid="ipr-method">{search ? t('ipRefresh.dialog.bodySearch') : t('ipRefresh.dialog.body')}</p>

      <label class="check">
        <input type="checkbox" bind:checked={aim} data-testid="ipr-aim" />
        <span>{t('ipRefresh.dialog.aimLabel')}</span>
      </label>
      <!-- An address reads left to right in every language: the field, then ".x.x". -->
      <div class="aim" class:off={!aim} dir="ltr">
        <input
          type="text"
          inputmode="decimal"
          dir="ltr"
          autocomplete="off"
          spellcheck="false"
          maxlength="8"
          placeholder="10.45"
          aria-label={t('ipRefresh.dialog.targetLabel')}
          aria-invalid={targetInvalid}
          aria-describedby="ipr-target-hint"
          disabled={!aim}
          bind:value={target}
          data-testid="ipr-target"
        />
        <bdi dir="ltr" class="tail" aria-hidden="true">{tail}</bdi>
      </div>
      <p id="ipr-target-hint" class="hint" class:bad={targetInvalid} role={targetInvalid ? 'alert' : undefined}>
        {targetInvalid ? t('ipRefresh.refuse.targetInvalid') : t('ipRefresh.dialog.targetHint')}
      </p>

      {#if aim}
        <div class="tries" role="group" aria-labelledby="ipr-tries-label">
          <span id="ipr-tries-label">{t('ipRefresh.dialog.tries')}</span>
          <button
            type="button"
            class="step"
            aria-label={t('ipRefresh.dialog.fewer')}
            disabled={tries <= 1}
            onclick={() => step(-1)}><Minus size={14} aria-hidden="true" /></button
          >
          <output class="count num" aria-live="polite" data-testid="ipr-tries">{formatNumber(tries, { digits })}</output>
          <button
            type="button"
            class="step"
            aria-label={t('ipRefresh.dialog.more')}
            disabled={tries >= MAX_TRIES}
            onclick={() => step(1)}><Plus size={14} aria-hidden="true" /></button
          >
        </div>
      {:else}
        <p class="hint">{t('ipRefresh.dialog.oneTry')}</p>
      {/if}
      <p class="hint" data-testid="ipr-estimate">{estimate}</p>

      <p class="note" class:strong={lastNotMatched} data-testid="ipr-work-note">{t('ipRefresh.dialog.workNote')}</p>

      {#if refusal}
        <p class="why" role="status" data-testid="ipr-refusal">{t(refusal)}</p>
      {/if}
      {#if error}
        <p class="why" role="alert" data-testid="ipr-error">{error}</p>
      {/if}

      <div class="actions">
        <button type="button" bind:this={cancelBtn} onclick={closeIpRefresh}>{t('app.cancel')}</button>
        <button type="button" class="primary" disabled={!canStart} onclick={start} data-testid="ipr-start"
          >{busy ? t('ipRefresh.dialog.starting') : t('ipRefresh.dialog.start')}</button
        >
      </div>
    </div>
  </div>
{/if}

<style>
  /* Below the title bar, so the window can still be moved and closed. */
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
    inline-size: min(480px, 100%);
    max-block-size: 100%;
    overflow-y: auto;
    animation: pop-in var(--t-enter) var(--ease);
  }
  h2 {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: var(--fs-section);
    margin-block: 0 8px;
  }
  p {
    margin-block: 0 8px;
  }
  .check {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-block: 12px 6px;
    font-weight: 500;
    cursor: pointer;
  }
  .aim {
    display: inline-flex;
    align-items: center;
    gap: 2px;
    margin-block-end: 4px;
  }
  .aim.off {
    opacity: 0.6;
  }
  .aim input {
    inline-size: 9ch;
    block-size: var(--control-height);
    padding-inline: 8px;
    border: 1px solid var(--field-border);
    border-radius: var(--radius-control);
    background: var(--surface);
    color: var(--text);
    font-variant-numeric: tabular-nums;
  }
  .aim input[aria-invalid='true'] {
    border-color: var(--q-poor);
  }
  .tail {
    color: var(--faint);
    font-variant-numeric: tabular-nums;
  }
  .hint {
    color: var(--muted);
    font-size: var(--fs-small);
  }
  .hint.bad {
    color: var(--pill-negative-ink);
  }
  .tries {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-block: 8px;
  }
  .tries #ipr-tries-label {
    margin-inline-end: 4px;
  }
  .step {
    display: grid;
    place-items: center;
    inline-size: var(--control-height);
    block-size: var(--control-height);
    padding: 0;
  }
  .count {
    min-inline-size: 2ch;
    text-align: center;
    font-weight: 500;
    font-variant-numeric: tabular-nums;
  }
  .note {
    padding-block: 8px;
    padding-inline: 10px;
    border-radius: var(--radius-control);
    background: var(--raised);
    color: var(--muted);
    font-size: var(--fs-small);
  }
  .note.strong {
    color: var(--text);
    border-inline-start: 3px solid var(--accent);
  }
  .why {
    color: var(--pill-negative-ink);
    background: var(--pill-negative-bg);
    border-radius: var(--radius-control);
    padding-block: 6px;
    padding-inline: 10px;
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: 8px;
    margin-block-start: var(--gap);
  }
</style>
