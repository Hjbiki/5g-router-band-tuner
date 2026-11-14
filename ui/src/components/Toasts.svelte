<script lang="ts">
  import { app, dismissToast } from '../lib/store.svelte'
  import { t } from '../lib/i18n'
  import X from '@lucide/svelte/icons/x'
  import CircleAlert from '@lucide/svelte/icons/circle-alert'
  import Info from '@lucide/svelte/icons/info'

  const TIMEOUT_MS = 8000

  /** Info toasts auto-dismiss after 8 s (held while hovered or focused); errors stay until dismissed. */
  function autoDismiss(node: HTMLElement, { id, kind }: { id: number; kind: string }) {
    if (kind === 'error') return
    let timer: ReturnType<typeof setTimeout> | undefined
    const start = () => {
      clearTimeout(timer)
      timer = setTimeout(() => dismissToast(id), TIMEOUT_MS)
    }
    const hold = () => clearTimeout(timer)
    const release = () => {
      if (!node.matches(':hover, :focus-within')) start()
    }
    node.addEventListener('mouseenter', hold)
    node.addEventListener('mouseleave', release)
    node.addEventListener('focusin', hold)
    node.addEventListener('focusout', release)
    start()
    return {
      destroy() {
        clearTimeout(timer)
        node.removeEventListener('mouseenter', hold)
        node.removeEventListener('mouseleave', release)
        node.removeEventListener('focusin', hold)
        node.removeEventListener('focusout', release)
      },
    }
  }
</script>

<div class="toasts" aria-live="polite">
  {#each app.toasts as toast (toast.id)}
    <div class="toast {toast.kind}" role={toast.kind === 'error' ? 'alert' : 'status'} use:autoDismiss={{ id: toast.id, kind: toast.kind }}>
      <span class="icon" aria-hidden="true">
        {#if toast.kind === 'error'}<CircleAlert size={16} />{:else}<Info size={16} />{/if}
      </span>
      <span class="text">{toast.text}</span>
      <button type="button" aria-label={t('app.dismiss')} onclick={() => dismissToast(toast.id)}>
        <X size={16} aria-hidden="true" />
      </button>
    </div>
  {/each}
</div>

<style>
  .toasts {
    position: fixed;
    inset-block-end: var(--pad);
    inset-inline-end: var(--pad);
    z-index: 60;
    display: flex;
    flex-direction: column;
    gap: 8px;
    max-inline-size: min(420px, calc(100vw - 2 * var(--pad)));
  }
  .toast {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    background: var(--surface);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: var(--radius-card);
    padding-block: 10px;
    padding-inline: 12px 6px;
    box-shadow: var(--shadow-pop);
    animation: pop-in var(--t-enter) var(--ease);
  }
  .icon {
    display: inline-grid;
    flex: none;
    margin-block-start: 2px;
    color: var(--muted);
  }
  .toast.error {
    background: var(--pill-negative-bg);
    color: var(--pill-negative-ink);
    border-color: transparent;
  }
  .toast.error .icon {
    color: inherit;
  }
  .text {
    flex: 1;
    min-inline-size: 0;
    overflow-wrap: anywhere;
  }
  button {
    all: unset;
    flex: none;
    display: inline-grid;
    place-items: center;
    inline-size: 24px;
    block-size: 24px;
    border-radius: var(--radius-chip);
    color: inherit;
    cursor: pointer;
  }
  button:hover {
    background: var(--active);
  }
  .toast.error button:hover {
    background: transparent;
    outline: 1px solid currentColor;
  }
  button:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
</style>
