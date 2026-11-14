<script lang="ts">
  import { onDestroy } from 'svelte'
  import { api } from '../lib/backend'
  import { app, pushToast } from '../lib/store.svelte'
  import { t } from '../lib/i18n'
  import { errorText } from '../lib/errors'
  import { formatNumber, isolate } from '../lib/format'
  import { changeOpen } from '../lib/bands'
  import { scanActive } from '../lib/scan.svelte'
  import RotateCcw from '@lucide/svelte/icons/rotate-ccw'

  // `label` names the opening button (the Refresh IP banner says "Restart router…"). The ids
  // are per instance, since the banner and About can be on screen together.
  let { label }: { label?: string } = $props()
  const uid = $props.id()

  let ticket: { id: string; deadline: number } | null = $state(null)
  let remaining = $state(0)
  let busy = $state(false)
  let timer: ReturnType<typeof setInterval> | undefined
  let cancelBtn: HTMLButtonElement | undefined = $state()

  const digits = $derived(app.settings?.digits ?? 'western')
  // Between the steps of a primary-band apply the change isn't finished either.
  const idle = $derived(!changeOpen(app.snapshot?.safeApply) && !scanActive())
  const canConfirm = $derived(ticket !== null && remaining > 0 && idle && !busy)

  function close() {
    clearInterval(timer)
    timer = undefined
    ticket = null
  }

  function tick() {
    if (!ticket) return
    remaining = Math.max(0, Math.ceil((ticket.deadline - Date.now()) / 1000))
    if (remaining === 0) {
      close()
      pushToast(t('reboot.expired'), 'error')
    }
  }

  async function request() {
    if (!idle || busy) return
    busy = true
    try {
      const r = await api.requestReboot()
      ticket = { id: r.id, deadline: Date.now() + r.expiresInS * 1000 }
      tick()
      clearInterval(timer)
      timer = setInterval(tick, 250)
    } catch (e) {
      pushToast(errorText(e), 'error')
    } finally {
      busy = false
    }
  }

  async function confirm() {
    if (!ticket || !canConfirm) return
    const id = ticket.id
    busy = true
    try {
      await api.confirmReboot(id)
      pushToast(t('reboot.rebooting'))
    } catch (e) {
      pushToast(errorText(e), 'error')
    } finally {
      busy = false
      close()
    }
  }

  $effect(() => {
    if (ticket) cancelBtn?.focus()
  })

  onDestroy(() => clearInterval(timer))
</script>

<div class="reboot">
  <button type="button" disabled={!idle || busy} aria-describedby={idle ? undefined : `${uid}-why`} onclick={request}
    ><RotateCcw size={16} aria-hidden="true" />{label ?? t('reboot.request')}</button
  >
  {#if !idle}<span id="{uid}-why" class="hint">{t('reboot.unavailable')}</span>{/if}
</div>

{#if ticket}
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div class="backdrop" role="presentation" onkeydown={(e) => e.key === 'Escape' && close()}>
    <div class="modal" role="dialog" aria-modal="true" aria-labelledby="{uid}-title" aria-describedby="{uid}-desc">
      <h2 id="{uid}-title">{t('reboot.title')}</h2>
      <p id="{uid}-desc">{t('reboot.description')}</p>
      <p class="count" aria-live="off">
        {t('reboot.expires', { seconds: isolate(formatNumber(remaining, { digits })) })}
      </p>
      {#if !idle}<p class="hint">{t('reboot.unavailable')}</p>{/if}
      <div class="actions">
        <button type="button" bind:this={cancelBtn} onclick={close}>{t('app.cancel')}</button>
        <button type="button" class="danger" disabled={!canConfirm} onclick={confirm}>{t('reboot.confirm')}</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .reboot {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    margin-block-start: 4px;
  }
  .reboot button {
    display: inline-flex;
    align-items: center;
    gap: 8px;
  }
  .hint {
    color: var(--muted);
    font-size: var(--fs-small);
  }
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
    inline-size: min(440px, 100%);
    animation: pop-in var(--t-enter) var(--ease);
  }
  h2 {
    font-size: var(--fs-section);
    margin-block: 0 8px;
  }
  p {
    margin-block: 0 8px;
  }
  .count {
    font-weight: 500;
    font-variant-numeric: tabular-nums;
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: 8px;
    margin-block-start: var(--gap);
  }
</style>
