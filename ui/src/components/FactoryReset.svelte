<script lang="ts">
  // The app's only way to erase the router: a red button beside Restart that opens a short
  // warning. No word has to be typed. What still stands in the
  // way: this model's call must be verified, the backend's 30 s ticket must be confirmed in the
  // warning, and it is off during a band change, a scan or a speed test. The backend repeats its
  // checks on both steps, and nothing here ever opens by itself. The confirm button comes alive
  // a second after the warning opens, so the second click of a double-click can't land on it.
  import { onDestroy } from 'svelte'
  import TriangleAlert from '@lucide/svelte/icons/triangle-alert'
  import { api } from '../lib/backend'
  import { app, pushToast } from '../lib/store.svelte'
  import { speed } from '../lib/speedtest.svelte'
  import { scanActive } from '../lib/scan.svelte'
  import { t, type MessageKey } from '../lib/i18n'
  import { errorText } from '../lib/errors'
  import { formatNumber, isolate } from '../lib/format'
  import { changeOpen } from '../lib/bands'

  const uid = $props.id()

  let ticket: { id: string; deadline: number } | null = $state(null)
  let remaining = $state(0)
  let busy = $state(false)
  /** How long the warning is open before its confirm button can be pressed. */
  const ARM_MS = 1000
  let armed = $state(false)
  let armTimer: ReturnType<typeof setTimeout> | undefined
  let timer: ReturnType<typeof setInterval> | undefined
  let cancelBtn: HTMLButtonElement | undefined = $state()

  const digits = $derived(app.settings?.digits ?? 'western')
  const snap = $derived(app.snapshot)
  const verified = $derived(snap?.capabilities?.factoryResetVerified === true)

  const blocked = $derived.by((): MessageKey | null => {
    if (!verified) return 'danger.notVerified'
    if (changeOpen(snap?.safeApply) || scanActive()) return 'danger.blockedBand'
    if (speed.running) return 'danger.blockedSpeed'
    if (snap?.session.kind !== 'loggedIn') return 'danger.blockedOffline'
    return null
  })

  function close() {
    clearInterval(timer)
    timer = undefined
    clearTimeout(armTimer)
    armed = false
    ticket = null
  }

  function tick() {
    if (!ticket) return
    remaining = Math.max(0, Math.ceil((ticket.deadline - Date.now()) / 1000))
    if (remaining === 0) {
      close()
      pushToast(t('danger.expired'), 'error')
    }
  }

  /** Opens the warning. Nothing is sent to the router: this only takes the backend's ticket. */
  async function open() {
    if (blocked || busy) return
    busy = true
    try {
      const r = await api.requestFactoryReset()
      ticket = { id: r.id, deadline: Date.now() + r.expiresInS * 1000 }
      armed = false
      clearTimeout(armTimer)
      armTimer = setTimeout(() => (armed = true), ARM_MS)
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
    if (!ticket || blocked || busy || !armed) return
    const id = ticket.id
    busy = true
    try {
      await api.confirmFactoryReset(id)
      pushToast(t('danger.started'))
    } catch (e) {
      pushToast(errorText(e), 'error')
    } finally {
      busy = false
      close()
    }
  }

  // A block that appears while the warning is open closes it; the backend would refuse anyway.
  $effect(() => {
    if (blocked && ticket) close()
  })
  // Cancel has the focus, so Enter never resets by accident.
  $effect(() => {
    if (ticket) cancelBtn?.focus()
  })
  onDestroy(() => {
    clearInterval(timer)
    clearTimeout(armTimer)
  })
</script>

<div class="reset">
  <button
    type="button"
    class="danger"
    disabled={blocked !== null || busy}
    aria-describedby={blocked ? `${uid}-why` : undefined}
    onclick={open}><TriangleAlert size={16} aria-hidden="true" />{t('danger.open')}</button
  >
  {#if blocked}<span id="{uid}-why" class="hint" data-testid="reset-blocked">{t(blocked)}</span>{/if}
</div>

{#if ticket}
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div class="backdrop" role="presentation" onkeydown={(e) => e.key === 'Escape' && close()}>
    <div class="modal" role="alertdialog" aria-modal="true" aria-labelledby="{uid}-title" aria-describedby="{uid}-desc">
      <h2 id="{uid}-title"><TriangleAlert size={20} aria-hidden="true" />{t('danger.title')}</h2>
      <p id="{uid}-desc">{t('danger.body')}</p>
      <p class="count" aria-live="off">
        {t('reboot.expires', { seconds: isolate(formatNumber(remaining, { digits })) })}
      </p>
      <div class="actions">
        <button type="button" bind:this={cancelBtn} onclick={close}>{t('app.cancel')}</button>
        <button type="button" class="now" disabled={busy || !armed} onclick={confirm}>{t('danger.confirm')}</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .reset {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    margin-block-start: 4px;
  }
  .reset button {
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
    border: 1.5px solid var(--danger-border);
    border-radius: var(--radius-card);
    box-shadow: var(--shadow-pop);
    padding-block: 20px;
    padding-inline: 24px;
    inline-size: min(440px, 100%);
    animation: pop-in var(--t-enter) var(--ease);
  }
  h2 {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: var(--fs-section);
    margin-block: 0 8px;
    color: var(--pill-negative-ink);
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
  .now {
    background: var(--danger-solid);
    border-color: var(--danger-solid);
    color: #ffffff;
    font-weight: 600;
  }
</style>
