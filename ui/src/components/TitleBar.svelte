<script lang="ts">
  import { onMount } from 'svelte'
  import { getCurrentWindow } from '@tauri-apps/api/window'
  import Minus from '@lucide/svelte/icons/minus'
  import Square from '@lucide/svelte/icons/square'
  import Copy from '@lucide/svelte/icons/copy'
  import X from '@lucide/svelte/icons/x'
  import { t } from '../lib/i18n'

  // The window has no native frame (tauri.conf.json "decorations": false), so this bar is the
  // only way to move, resize and close it. Closing asks the window to close: Rust turns that
  // CloseRequested into "hide to tray", exactly like the old native close button.
  let maximized = $state(false)

  function win() {
    try {
      return getCurrentWindow()
    } catch {
      return null
    }
  }

  onMount(() => {
    const w = win()
    if (!w) return
    let off: (() => void) | undefined
    let gone = false
    const sync = () =>
      void w
        .isMaximized()
        .then((m) => (maximized = m))
        .catch(() => {})
    sync()
    w.onResized(sync)
      .then((u) => (gone ? u() : (off = u)))
      .catch(() => {})
    return () => {
      gone = true
      off?.()
    }
  })

  const minimise = () => void win()?.minimize().catch(() => {})
  const toggleMaximise = () => void win()?.toggleMaximize().catch(() => {})
  const close = () => void win()?.close().catch(() => {})
</script>

<header class="titlebar" aria-label={t('shell.titleBar')} data-tauri-drag-region>
  <!-- The mark lets presses through to the bar: its child rects carry no drag attribute, so a press on them would not move the window. -->
  <svg class="mark" width="16" height="16" viewBox="0 0 22 22" aria-hidden="true" pointer-events="none">
    <rect x="1" y="13" width="4" height="8" rx="1"></rect>
    <rect x="7" y="9" width="4" height="12" rx="1"></rect>
    <rect class="hi" x="13" y="5" width="4" height="16" rx="1"></rect>
    <rect x="19" y="1" width="2" height="20" rx="1"></rect>
  </svg>
  <span class="name" data-tauri-drag-region>{t('app.title')}</span>
  <span class="grow" data-tauri-drag-region></span>
  <button type="button" class="cap" aria-label={t('shell.minimise')} title={t('shell.minimise')} onclick={minimise}>
    <Minus size={16} aria-hidden="true" />
  </button>
  <button
    type="button"
    class="cap"
    aria-label={maximized ? t('shell.restore') : t('shell.maximise')}
    title={maximized ? t('shell.restore') : t('shell.maximise')}
    onclick={toggleMaximise}
  >
    {#if maximized}<Copy size={13} aria-hidden="true" />{:else}<Square size={13} aria-hidden="true" />{/if}
  </button>
  <button type="button" class="cap close" aria-label={t('shell.close')} title={t('shell.close')} onclick={close}>
    <X size={16} aria-hidden="true" />
  </button>
</header>

<style>
  /* In RTL the flex row runs right to left, so the window buttons sit on the left. */
  .titlebar {
    block-size: var(--titlebar-height);
    flex: none;
    display: flex;
    align-items: center;
    padding-inline-start: 14px;
    background: var(--ground);
    color: var(--muted);
    user-select: none;
    -webkit-user-select: none;
  }
  .mark {
    flex: none;
    pointer-events: none;
    fill: var(--text);
  }
  .mark .hi {
    fill: var(--accent);
  }
  .name {
    margin-inline-start: 10px;
    font-size: var(--fs-small);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .grow {
    flex: 1;
    align-self: stretch;
  }
  .cap {
    all: unset;
    flex: none;
    inline-size: 46px;
    block-size: var(--titlebar-height);
    display: grid;
    place-items: center;
    color: var(--muted);
  }
  .cap:hover {
    background: var(--active);
    color: var(--text);
  }
  .cap.close:hover {
    background: var(--window-close);
    color: var(--window-close-ink);
  }
  .cap:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }
</style>
