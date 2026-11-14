<script lang="ts">
  // "Use this APN": says what happens, shows the exact request, and starts the switch only on
  // the second click. The backend undoes it by itself when the new APN gives no data.
  import Waypoints from '@lucide/svelte/icons/waypoints'
  import { api } from '../lib/backend'
  import { app } from '../lib/store.svelte'
  import { t } from '../lib/i18n'
  import { errorText } from '../lib/errors'
  import { isolateText as fsi } from '../lib/format'
  import { apnSwitchRefusal } from '../lib/apn'
  import { scanActive } from '../lib/scan.svelte'
  import { speed } from '../lib/speedtest.svelte'
  import type { ApnProfile } from '../lib/bindings/ApnProfile'
  import type { ApplyPreview } from '../lib/bindings/ApplyPreview'
  import SendPreview from './SendPreview.svelte'

  let {
    profile,
    current,
    onclose,
  }: { profile: ApnProfile; current: ApnProfile | null; onclose: () => void } = $props()

  let busy = $state(false)
  let error: string | null = $state(null)
  let preview: ApplyPreview | null = $state(null)
  let previewState: 'loading' | 'ready' | 'failed' = $state('loading')
  let cancelBtn: HTMLButtonElement | undefined = $state()

  const name = $derived(profile.name || t('network.apn.unnamed'))
  const currentName = $derived(current ? current.name || t('network.apn.unnamed') : t('network.apn.current'))
  const refusal = $derived(apnSwitchRefusal(app.snapshot, { scan: scanActive(), speedTest: speed.running }))

  $effect(() => {
    const { index, fingerprint } = profile
    previewState = 'loading'
    api.apnPreview(index, fingerprint).then(
      (p) => {
        preview = p
        previewState = 'ready'
      },
      () => (previewState = 'failed'),
    )
    queueMicrotask(() => cancelBtn?.focus())
  })

  async function start() {
    if (busy || refusal) return
    busy = true
    error = null
    try {
      await api.apnSwitch(profile.index, profile.fingerprint)
      onclose()
    } catch (e) {
      error = errorText(e)
    } finally {
      busy = false
    }
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div class="backdrop" role="presentation" onkeydown={(e) => e.key === 'Escape' && onclose()}>
  <div class="modal" role="dialog" aria-modal="true" aria-labelledby="apn-sw-title" aria-describedby="apn-sw-desc">
    <h2 id="apn-sw-title"><Waypoints size={18} aria-hidden="true" />{t('network.apn.switch.title')}</h2>
    <p id="apn-sw-desc">
      {t('network.apn.switch.body', { name: fsi(name), current: fsi(currentName) })}
    </p>
    <ol class="steps">
      <li>{t('network.apn.switch.step1')}</li>
      <li>{t('network.apn.switch.step2')}</li>
      <li>{t('network.apn.switch.step3', { current: fsi(currentName) })}</li>
    </ol>
    <SendPreview {preview} state={previewState} />
    {#if refusal}
      <p class="why" role="status" data-testid="apn-sw-refusal">{t(refusal)}</p>
    {/if}
    {#if error}
      <p class="why" role="alert" data-testid="apn-sw-error">{error}</p>
    {/if}
    <div class="actions">
      <button type="button" bind:this={cancelBtn} onclick={onclose}>{t('app.cancel')}</button>
      <button
        type="button"
        class="primary"
        disabled={busy || refusal !== null}
        onclick={start}
        data-testid="apn-sw-start">{busy ? t('network.apn.switch.starting') : t('network.apn.switch.start')}</button
      >
    </div>
  </div>
</div>

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
  .steps {
    margin-block: 0 12px;
    padding-inline-start: 20px;
    color: var(--muted);
    font-size: var(--fs-small);
  }
  .why {
    color: var(--pill-negative-ink);
    background: var(--pill-negative-bg);
    border-radius: var(--radius-control);
    padding-block: 6px;
    padding-inline: 10px;
    margin-block-start: 8px;
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: 8px;
    margin-block-start: var(--gap);
  }
</style>
