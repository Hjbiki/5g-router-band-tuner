<script lang="ts">
  // Notify only: opens the release page, and never downloads or installs anything.
  import { onMount } from 'svelte'
  import ArrowUpCircle from '@lucide/svelte/icons/circle-arrow-up'
  import ExternalLink from '@lucide/svelte/icons/external-link'
  import RefreshCw from '@lucide/svelte/icons/refresh-cw'
  import { api } from '../lib/backend'
  import { app, updateSettings } from '../lib/store.svelte'
  import { t } from '../lib/i18n'
  import { errorText } from '../lib/errors'
  import { ensureUpdates, updates } from '../lib/updates.svelte'

  onMount(() => {
    void ensureUpdates()
  })

  const available = $derived(updates.view?.available ?? null)
  const asked = $derived(app.settings ? app.settings.updateCheck !== null : true)
  let checking = $state(false)
  // What the last press of the button found; null before one.
  let result: 'upToDate' | 'failed' | null = $state(null)
  let failure = $state('')

  async function check() {
    checking = true
    result = null
    try {
      const v = await api.checkForUpdates()
      updates.view = v
      if (!v.available) result = 'upToDate'
    } catch (e) {
      failure = errorText(e)
      result = 'failed'
    } finally {
      checking = false
    }
  }

  async function dismiss() {
    if (available) updates.view = await api.dismissUpdate(available.version)
  }

  const answer = (allow: boolean) => updateSettings((s) => ({ ...s, updateCheck: allow })).catch(() => {})
</script>

<div class="card">
  <h2><ArrowUpCircle size={18} aria-hidden="true" />{t('updates.title')}</h2>

  {#if !asked}
    <div class="ask" data-testid="update-question">
      <p>{t('updates.question')}</p>
      <p class="muted">{t('updates.questionHint')}</p>
      <div class="row">
        <button type="button" onclick={() => answer(true)}>{t('updates.allow')}</button>
        <button type="button" onclick={() => answer(false)}>{t('updates.notNow')}</button>
      </div>
    </div>
  {/if}

  {#if available}
    <div class="notice" role="status" data-testid="update-notice">
      <strong><bdi dir="ltr">{t('updates.available', { version: available.version })}</bdi></strong>
      {#if available.summary}<p dir="auto">{available.summary}</p>{/if}
      <div class="row">
        <button type="button" class="primary" onclick={() => api.openReleasePage().catch(() => {})}>
          <ExternalLink size={16} aria-hidden="true" />{t('updates.open')}
        </button>
        <button type="button" onclick={dismiss}>{t('app.dismiss')}</button>
      </div>
    </div>
  {/if}

  <div class="row">
    <button type="button" disabled={checking} onclick={check}>
      <RefreshCw size={16} aria-hidden="true" />{checking ? t('updates.checking') : t('updates.check')}
    </button>
    {#if result === 'upToDate'}
      <span class="muted" role="status">{t('updates.upToDate')}</span>
    {:else if result === 'failed'}
      <span class="muted" role="status">{failure}</span>
    {/if}
  </div>
</div>

<style>
  .card {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius-card);
    padding-block: var(--card-pad-block);
    padding-inline: var(--card-pad-inline);
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-inline-size: 0;
  }
  p {
    margin: 0;
  }
  .muted {
    color: var(--muted);
    font-size: var(--fs-small);
  }
  .ask,
  .notice {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding-block: 10px;
    padding-inline: 12px;
    border-radius: var(--radius-control);
    background: var(--raised);
  }
  strong {
    font-weight: 600;
  }
  .row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
  }
  button {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    min-block-size: var(--control-height);
    font-weight: 500;
  }
</style>
