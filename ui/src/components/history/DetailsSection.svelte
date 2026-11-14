<script lang="ts">
  // One "More details" section of the History page: a native details element, closed until opened,
  // that remembers being opened on this PC. Its body is only mounted while it's open, so a closed
  // chart or table costs nothing. A short preview can stand in for the body while it's closed.
  import type { Snippet } from 'svelte'
  import ChevronDown from '@lucide/svelte/icons/chevron-down'
  import { t } from '../../lib/i18n'
  import { sectionOpen, setSectionOpen, type HistorySection } from '../../lib/history.svelte'

  let {
    id,
    title,
    icon: Icon,
    preview = null,
    children,
  }: { id: HistorySection; title: string; icon: typeof ChevronDown; preview?: string | null; children: Snippet } = $props()

  const open = $derived(sectionOpen(id))

  function onToggle(e: Event) {
    const now = (e.currentTarget as HTMLDetailsElement).open
    if (now !== open) setSectionOpen(id, now)
  }
</script>

<details class="card" {open} ontoggle={onToggle} data-testid="section-{id}">
  <summary id="hs-sec-{id}">
    <h3><Icon size={18} aria-hidden="true" />{title}</h3>
    {#if preview && !open}<span class="preview">{preview}</span>{/if}
    <span class="toggle" aria-hidden="true">{t(open ? 'events.hide' : 'events.show')}<ChevronDown size={16} /></span>
  </summary>
  {#if open}
    <div class="body">{@render children()}</div>
  {/if}
</details>

<style>
  .card {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius-card);
    padding-block: var(--card-pad-block);
    padding-inline: var(--card-pad-inline);
  }
  summary {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 4px 12px;
    cursor: pointer;
    list-style: none;
  }
  summary::-webkit-details-marker {
    display: none;
  }
  summary:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  h3 {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    margin: 0;
    font-size: var(--fs-section);
    font-weight: 600;
    line-height: 24px;
  }
  h3 :global(svg) {
    color: var(--accent);
  }
  .preview {
    color: var(--muted);
    font-size: var(--fs-small);
  }
  /* Shows that the heading opens: a word and a chevron that turns when open. */
  .toggle {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    margin-inline-start: auto;
    color: var(--muted);
    font-size: var(--fs-small);
  }
  summary:hover .toggle {
    color: var(--text);
  }
  .toggle :global(svg) {
    transition: transform var(--t-fast) var(--ease);
  }
  details[open] > summary .toggle :global(svg) {
    transform: rotate(180deg);
  }
  @media (prefers-reduced-motion: reduce) {
    .toggle :global(svg) {
      transition: none;
    }
  }
  .body {
    display: flex;
    flex-direction: column;
    gap: 12px;
    margin-block-start: 12px;
  }
</style>
