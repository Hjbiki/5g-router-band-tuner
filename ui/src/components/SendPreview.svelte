<script lang="ts">
  import ChevronDown from '@lucide/svelte/icons/chevron-down'
  import type { ApplyPreview } from '../lib/bindings/ApplyPreview'
  import { t } from '../lib/i18n'

  // The backend's own rendering of what Apply will send, folded away by default.
  let { preview, state }: { preview: ApplyPreview | null; state: 'loading' | 'ready' | 'failed' } = $props()
</script>

<details class="preview">
  <summary>
    <ChevronDown class="chev" size={14} aria-hidden="true" />
    <span>{t('bands.previewSummary')}</span>
  </summary>
  <section class="body" aria-label={t('bands.previewTitle')}>
    {#if preview}
      <h4>{t('bands.previewFields')}</h4>
      <dl dir="ltr">
        {#each preview.fields as [name, value], i (i)}
          <dt>{name}</dt>
          <dd>{value}</dd>
        {/each}
      </dl>
      <h4>{t('bands.previewBody')}</h4>
      <pre data-testid="preview-body" dir="ltr" class:stale={state !== 'ready'}>{preview.body}</pre>
    {/if}
    {#if state === 'loading'}
      <p class="hint" role="status">{t('bands.previewLoading')}</p>
    {:else if state === 'failed'}
      <p class="hint err" role="alert">{t('bands.reason.previewFailed')}</p>
    {/if}
  </section>
</details>

<style>
  .preview {
    font-size: var(--fs-small);
  }
  summary {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    cursor: pointer;
    color: var(--text);
    list-style: none;
    border-radius: var(--radius-chip);
  }
  summary::-webkit-details-marker {
    display: none;
  }
  summary :global(.chev) {
    color: var(--muted);
    transform: rotate(-90deg);
    transition: transform 120ms ease;
  }
  :global([dir='rtl']) summary :global(.chev) {
    transform: rotate(90deg);
  }
  .preview[open] summary :global(.chev) {
    transform: none;
  }
  .body {
    margin-block-start: 8px;
  }
  h4 {
    margin-block: 8px 4px;
    font-size: var(--fs-small);
    font-weight: 500;
    color: var(--muted);
  }
  h4:first-child {
    margin-block-start: 0;
  }
  dl {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 2px 12px;
    margin: 0;
    font-family: ui-monospace, Consolas, monospace;
    font-size: var(--fs-small);
    text-align: start;
  }
  dd {
    margin: 0;
    overflow-wrap: anywhere;
  }
  pre {
    margin: 0;
    padding: 8px;
    overflow: auto;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    font-family: ui-monospace, Consolas, monospace;
    font-size: var(--fs-small);
    background: var(--raised);
    color: var(--muted);
    border-radius: var(--radius-chip);
    text-align: start;
  }
  pre.stale {
    opacity: 0.5;
  }
  .hint {
    margin-block: 6px 0;
    color: var(--muted);
  }
  .err {
    color: var(--q-poor);
  }
  @media (prefers-reduced-motion: reduce) {
    summary :global(.chev) {
      transition: none;
    }
  }
</style>
