<script lang="ts">
  // The History page's CSV export: which records, whether to add the cell numbers or the IP range,
  // and the header
  // language. The backend builds the file, runs its privacy check and asks where to save it.
  import FileDown from '@lucide/svelte/icons/file-down'
  import X from '@lucide/svelte/icons/x'
  import { api } from '../../lib/backend'
  import { app, pushToast } from '../../lib/store.svelte'
  import { t, currentLanguage } from '../../lib/i18n'
  import { errorText } from '../../lib/errors'
  import { csvLabels } from '../../lib/historyCsv'
  import { history, rangeBounds } from '../../lib/history.svelte'
  import type { ExportDataset } from '../../lib/bindings/ExportDataset'
  import { formatDay, formatTime } from './TimeChart.svelte'

  let { onclose }: { onclose?: () => void } = $props()

  const DATASETS: readonly ExportDataset[] = ['hourly', 'speedTests', 'minutes', 'events', 'all']

  const digits = $derived(app.settings?.digits ?? 'western')
  const lang = $derived(currentLanguage())
  // Neither box is remembered: each export starts with them off.
  let dataset = $state<ExportDataset>('hourly')
  let cellNumbers = $state(false)
  let ipRange = $state(false)
  let english = $state(false)
  let busy = $state(false)
  let error: string | null = $state(null)

  const bounds = $derived(history.loadedRange ?? rangeBounds(history.range, Date.now()))
  const when = (ms: number) => `${formatDay(ms, lang, digits)} ${formatTime(ms, lang, digits)}`

  async function save() {
    busy = true
    error = null
    try {
      const out = await api.exportHistoryCsv(
        { range: bounds, dataset, includeCellNumbers: cellNumbers, includeIpRange: ipRange },
        csvLabels(english || lang === 'en'),
      )
      if (out.kind === 'saved') {
        pushToast(t('history.export.saved', { files: out.files.join(', ') }))
        onclose?.()
      }
    } catch (e) {
      error = errorText(e)
    } finally {
      busy = false
    }
  }
</script>

<section class="panel" aria-labelledby="hx-title" data-testid="history-export">
  <div class="head">
    <h2 id="hx-title"><FileDown size={18} aria-hidden="true" />{t('history.export.title')}</h2>
    {#if onclose}
      <button type="button" class="icon" aria-label={t('app.close')} onclick={onclose} disabled={busy}>
        <X size={16} aria-hidden="true" />
      </button>
    {/if}
  </div>
  <p class="muted">{t('history.export.intro')}</p>

  <fieldset>
    <legend>{t('history.export.datasetLabel')}</legend>
    {#each DATASETS as d (d)}
      <label class="choice">
        <input type="radio" name="history-export-dataset" value={d} bind:group={dataset} disabled={busy} />
        <span>
          <span class="name">{t(`history.export.dataset.${d}`)}</span>
          <span class="muted small">{t(`history.export.datasetHint.${d}`)}</span>
        </span>
      </label>
    {/each}
  </fieldset>

  <p class="small">{t('history.export.range', { from: when(bounds.fromMs), to: when(bounds.toMs) })}</p>

  <label class="choice">
    <input type="checkbox" bind:checked={cellNumbers} disabled={busy} aria-describedby="hx-cells-hint" />
    <span>
      <span class="name">{t('history.export.cellNumbers')}</span>
      <span id="hx-cells-hint" class="muted small">{t('history.export.cellNumbersHint')}</span>
    </span>
  </label>
  <label class="choice">
    <input type="checkbox" bind:checked={ipRange} disabled={busy} aria-describedby="hx-ip-hint" />
    <span>
      <span class="name">{t('history.export.ipRange')}</span>
      <span id="hx-ip-hint" class="muted small">{t('history.export.ipRangeHint')}</span>
    </span>
  </label>
  <p class="muted small">{t('history.export.neverExported')}</p>

  {#if lang !== 'en'}
    <label class="choice">
      <input type="checkbox" bind:checked={english} disabled={busy} aria-describedby="hx-english-hint" />
      <span>
        <span class="name">{t('history.export.english')}</span>
        <span id="hx-english-hint" class="muted small">{t('history.export.englishHint')}</span>
      </span>
    </label>
  {/if}

  <div class="row">
    <button type="button" class="primary" onclick={save} disabled={busy}>
      {busy ? t('history.export.saving') : dataset === 'all' ? t('history.export.chooseFolder') : t('history.export.save')}
    </button>
    {#if onclose}
      <button type="button" class="secondary" onclick={onclose} disabled={busy}>{t('app.cancel')}</button>
    {/if}
  </div>
  {#if error}<p class="err" role="alert">{error}</p>{/if}
</section>

<style>
  .panel {
    display: flex;
    flex-direction: column;
    gap: 10px;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius-card);
    padding-block: var(--card-pad-block);
    padding-inline: var(--card-pad-inline);
  }
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }
  h2 {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 0;
  }
  p {
    margin: 0;
    max-inline-size: var(--measure);
  }
  fieldset {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin: 0;
    padding: 0;
    border: 0;
  }
  legend {
    padding: 0;
    margin-block-end: 4px;
    font-weight: 500;
  }
  .choice {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    cursor: pointer;
  }
  .choice input {
    margin-block-start: 3px;
  }
  .choice > span {
    display: flex;
    flex-direction: column;
  }
  .muted {
    color: var(--muted);
  }
  .small {
    font-size: var(--fs-small);
  }
  .row {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  .icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    inline-size: 28px;
    block-size: 28px;
    padding: 0;
  }
  .err {
    color: var(--pill-negative-ink);
  }
</style>
