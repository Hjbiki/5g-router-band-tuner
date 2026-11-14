<script lang="ts">
  // A band set as one tag per band, each striped with its kind (FDD, TDD or SDL), joined by "+".
  // The kind is also in each tag's title and in screen-reader text.
  import { duplexOf, supportedBands } from '../../lib/bands'

  let { bands, allLabel, total = 0 }: { bands: string; allLabel?: string; total?: number } = $props()
  const list = $derived(supportedBands(bands))
  const KIND = { fdd: 'FDD', tdd: 'TDD', sdl: 'SDL' } as const
</script>

{#if allLabel && total > 0 && list.length >= total}
  <span class="tags all">{allLabel}</span>
{:else}
  <span class="tags"
    >{#each list as b, i (b)}{#if i > 0}<span class="plus" aria-hidden="true">+</span>{/if}<bdi
        dir="ltr"
        class="btag duplex-{duplexOf(b)}"
        title={KIND[duplexOf(b)]}>B{b}<span class="sr-only"> {KIND[duplexOf(b)]}</span></bdi
      >{/each}</span
  >
{/if}

<style>
  .tags {
    display: inline-flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 2px 4px;
    vertical-align: middle;
  }
  .btag {
    display: inline-block;
    padding-block: 0;
    padding-inline: 5px 4px;
    border-inline-start: 3px solid var(--duplex, var(--border));
    border-radius: 2px;
    font-weight: inherit;
    font-variant-numeric: tabular-nums;
    line-height: 1.35;
  }
  .plus {
    color: var(--faint);
    font-size: 0.85em;
  }
</style>
