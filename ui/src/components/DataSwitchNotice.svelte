<script lang="ts">
  // Mobile data that may be off without the app turning it back on by itself: off with no record
  // of the app doing it, a record the app won't use (another router, or unreadable), or a Refresh
  // IP run that can't log in to turn it back on. The button sends one "on" and reads it back.
  import WifiOff from '@lucide/svelte/icons/wifi-off'
  import { api } from '../lib/backend'
  import { app, pushToast } from '../lib/store.svelte'
  import { t, type MessageKey } from '../lib/i18n'
  import { errorText } from '../lib/errors'
  import { isolate } from '../lib/format'

  const snap = $derived(app.snapshot)
  const loggedIn = $derived(snap?.session.kind === 'loggedIn')
  const run = $derived(snap?.ipRefresh ?? null)

  const kind = $derived.by((): 'stuck' | 'blocked' | 'off' | null => {
    if (!snap) return null
    // A run that turned data off and can't log back in to turn it on.
    if (run?.active && !loggedIn && run.phase !== 'turningOff') return 'stuck'
    if (run?.journalBlocked) return 'blocked'
    if (snap.dataSwitchOff && loggedIn) return 'off'
    return null
  })
  const TEXT: Record<'stuck' | 'blocked' | 'off', [MessageKey, MessageKey]> = {
    stuck: ['ipRefresh.notice.stuckTitle', 'ipRefresh.notice.stuckBody'],
    blocked: ['ipRefresh.notice.blockedTitle', 'ipRefresh.notice.blockedBody'],
    off: ['ipRefresh.notice.offTitle', 'ipRefresh.notice.offBody'],
  }
  // The run turns data on itself once logged in; the button is for the other two.
  const canPress = $derived(kind !== 'stuck' && loggedIn && !run?.active)

  let busy = $state(false)
  async function turnOn() {
    if (busy || !canPress) return
    busy = true
    try {
      await api.dataSwitchOn()
      pushToast(t('ipRefresh.notice.turnedOn'))
    } catch (e) {
      pushToast(errorText(e), 'error')
    } finally {
      busy = false
    }
  }
</script>

{#if kind}
  <section class="notice" role="alert" aria-labelledby="data-off-title" data-testid="data-switch-notice" data-kind={kind}>
    <h2 id="data-off-title"><WifiOff size={18} aria-hidden="true" />{t(TEXT[kind][0])}</h2>
    <p>{t(TEXT[kind][1], { host: isolate(app.settings?.host ?? '192.168.8.1') })}</p>
    {#if kind !== 'stuck'}
      <div class="row">
        <button type="button" class="primary" disabled={busy || !canPress} onclick={turnOn}
          >{busy ? t('ipRefresh.notice.turningOn') : t('ipRefresh.notice.turnOn')}</button
        >
      </div>
    {/if}
  </section>
{/if}

<style>
  .notice {
    border: 1px solid var(--border);
    border-inline-start: 4px solid var(--q-poor);
    border-radius: var(--radius-card);
    background: var(--surface);
    padding-block: 14px;
    padding-inline: 18px;
  }
  h2 {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: var(--fs-section);
    margin-block: 0 6px;
  }
  h2 :global(svg) {
    flex: none;
    color: var(--q-poor);
  }
  p {
    margin-block: 0 8px;
  }
  .row {
    margin-block: 4px 0;
  }
</style>
