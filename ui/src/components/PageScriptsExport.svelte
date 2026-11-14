<script lang="ts">
  import { api } from '../lib/backend'
  import { app, pushToast } from '../lib/store.svelte'
  import { t } from '../lib/i18n'
  import { errorText } from '../lib/errors'
  import { formatNumber } from '../lib/format'
  import type { PageScriptsSummary } from '../lib/bindings/PageScriptsSummary'
  import FileCode from '@lucide/svelte/icons/file-code'

  // What the prepared zip holds; shown before anything is saved.
  let summary: PageScriptsSummary | null = $state(null)
  let busy = $state(false)
  let error: string | null = $state(null)

  const digits = $derived(app.settings?.digits ?? 'western')
  const loggedIn = $derived(app.snapshot?.session.kind === 'loggedIn')

  function kb(bytes: number): string {
    return formatNumber(Math.max(1, Math.round(bytes / 1024)), { digits })
  }

  async function prepare() {
    busy = true
    error = null
    try {
      summary = await api.preparePageScripts()
    } catch (e) {
      error = errorText(e)
    } finally {
      busy = false
    }
  }

  async function save() {
    busy = true
    error = null
    try {
      if (await api.savePageScripts()) {
        pushToast(t('pageScripts.saved'))
        summary = null
      }
    } catch (e) {
      error = errorText(e)
    } finally {
      busy = false
    }
  }
</script>

<div class="page-scripts">
  <h3><FileCode size={16} aria-hidden="true" />{t('pageScripts.title')}</h3>
  <p class="body">{t('pageScripts.body')}</p>
  <p class="muted">{t('pageScripts.private')}</p>
  {#if summary === null}
    <div class="row">
      <button type="button" onclick={prepare} disabled={busy || !loggedIn}>
        {busy ? t('pageScripts.reading') : t('pageScripts.read')}
      </button>
    </div>
    {#if !loggedIn}<p class="muted">{t('pageScripts.needLogin')}</p>{/if}
  {:else}
    <div class="review" role="region" aria-labelledby="scripts-review">
      <strong id="scripts-review">{t('pageScripts.reviewTitle')}</strong>
      <ul class="facts">
        <li>{t('pageScripts.files', { count: formatNumber(summary.files, { digits }), size: kb(summary.bytes) })}</li>
        <li>{t('pageScripts.zip', { size: kb(summary.zipBytes) })}</li>
        <li>{t('pageScripts.missing', { count: formatNumber(summary.missing.length, { digits }) })}</li>
        <li>{t('pageScripts.skipped', { count: formatNumber(summary.skipped.length, { digits }) })}</li>
      </ul>
      {#if summary.stopped}<p class="muted">{t('pageScripts.stopped')}</p>{/if}
      {#if summary.skipped.length}
        <details>
          <summary>{t('pageScripts.skippedList')}</summary>
          <ul class="paths" dir="ltr">
            {#each summary.skipped as s (s.path)}
              <li><code>{s.path}</code> <span class="muted" dir="auto">{t(`pageScripts.reason.${s.reason}`)}</span></li>
            {/each}
          </ul>
        </details>
      {/if}
      <div class="row">
        <button type="button" class="primary" onclick={save} disabled={busy}>{t('pageScripts.save')}</button>
        <button type="button" onclick={() => (summary = null)} disabled={busy}>{t('app.cancel')}</button>
      </div>
    </div>
  {/if}
  {#if error}<p class="err" role="alert">{error}</p>{/if}
</div>

<style>
  h3 {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 0 0 8px;
    font-size: var(--fs-body);
  }
  p {
    margin: 0 0 8px;
  }
  .review {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .facts,
  .paths {
    margin: 0;
    padding-inline-start: 20px;
  }
  .paths {
    max-block-size: 160px;
    overflow: auto;
    font-size: var(--fs-small);
  }
  code {
    font-family: ui-monospace, 'Cascadia Mono', Consolas, monospace;
    overflow-wrap: anywhere;
  }
  .row {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  .muted {
    color: var(--muted);
  }
  .err {
    margin: 8px 0 0;
    color: var(--q-poor);
  }
</style>
