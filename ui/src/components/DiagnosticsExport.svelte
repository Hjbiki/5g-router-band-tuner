<script lang="ts">
  import { api } from '../lib/backend'
  import { pushToast } from '../lib/store.svelte'
  import { t } from '../lib/i18n'
  import { errorText } from '../lib/errors'
  import FileDown from '@lucide/svelte/icons/file-down'

  // The redacted text exactly as the backend will write it; shown before saving.
  let text: string | null = $state(null)
  let busy = $state(false)
  let error: string | null = $state(null)

  async function prepare() {
    busy = true
    error = null
    try {
      text = await api.prepareDiagnostics()
    } catch (e) {
      error = errorText(e)
    } finally {
      busy = false
    }
  }

  async function save() {
    busy = true
    try {
      if (await api.saveDiagnostics()) {
        pushToast(t('diagnostics.saved'))
        text = null
      }
    } catch (e) {
      error = errorText(e)
    } finally {
      busy = false
    }
  }
</script>


<p class="body">{t('diagnostics.body')}</p>
{#if text === null}
  <div class="row">
    <button type="button" onclick={prepare} disabled={busy}>
      <FileDown size={16} aria-hidden="true" />
      {busy ? t('diagnostics.preparing') : t('diagnostics.export')}
    </button>
  </div>
{:else}
  <div class="review" role="region" aria-labelledby="diag-review">
    <strong id="diag-review">{t('diagnostics.reviewTitle')}</strong>
    <p class="muted">{t('diagnostics.reviewBody')}</p>
    <textarea dir="ltr" readonly rows="14" aria-labelledby="diag-review" value={text}></textarea>
    <div class="row">
      <button type="button" class="primary" onclick={save} disabled={busy}>{t('diagnostics.save')}</button>
      <button type="button" onclick={() => (text = null)} disabled={busy}>{t('app.cancel')}</button>
    </div>
  </div>
{/if}
{#if error}<p class="err" role="alert">{error}</p>{/if}

<style>
  .body {
    margin: 0 0 8px;
  }
  .review {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .review p {
    margin: 0;
  }
  textarea {
    inline-size: 100%;
    box-sizing: border-box;
    resize: vertical;
    font-family: ui-monospace, 'Cascadia Mono', Consolas, monospace;
    color: var(--text);
    margin: 0;
    padding: 10px;
    background: var(--raised);
    border: 1px solid var(--border);
    border-radius: var(--radius-control);
    font-size: var(--fs-small);
    white-space: pre;
    overflow: auto;
  }
  .row {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  .row button {
    display: inline-flex;
    align-items: center;
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
