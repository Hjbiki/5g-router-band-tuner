<script lang="ts">
  import type { Snippet } from 'svelte'
  import { t } from '../../lib/i18n'
  import InfoTip from './InfoTip.svelte'

  // One label and value line inside a <dl>; the explanation opens under the whole line.
  let {
    label,
    term = label,
    help,
    children,
  }: { label: string; term?: string; help?: string; children: Snippet } = $props()
</script>

<div class="fact">
  <dt>
    <span>{label}</span>
    {#if help}<InfoTip label={t('metrics.whatIs', { name: term })} text={help} />{/if}
  </dt>
  <dd>{@render children()}</dd>
</div>

<style>
  .fact {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 2px 6px;
    min-block-size: 30px;
  }
  /* The label, button, sentence and value all sit in the line's own flex row. */
  dt {
    display: contents;
    color: var(--muted);
  }
  dt span {
    color: var(--muted);
  }
  dd {
    margin: 0;
    margin-inline-start: auto;
    text-align: end;
    font-variant-numeric: tabular-nums;
  }
</style>
