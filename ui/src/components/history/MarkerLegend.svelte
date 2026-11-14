<script lang="ts">
  // Legend entries for the upright lines on a history chart: one per kind shown, each with its
  // own top mark and words, so colour never carries the meaning alone.
  import { t } from '../../lib/i18n'
  import type { ChartMarker } from './TimeChart.svelte'
  import MarkerGlyph from './MarkerGlyph.svelte'

  let { markers }: { markers: ChartMarker[] } = $props()
  const ORDER = ['kept', 'reverted', 'applied', 'reboot', 'appStart'] as const
  type Kind = (typeof ORDER)[number]
  const kinds = $derived(ORDER.filter((k) => markers.some((m) => m.kind === k)))
  const label = (k: Kind) => t(`history.signal.legendMarker.${k}`)
</script>

{#each kinds as k (k)}
  <li data-legend-marker={k}>
    <svg width="12" height="18" aria-hidden="true">
      <line class="mark {k}" x1="6" x2="6" y1="9" y2="18" />
      <MarkerGlyph kind={k} cx={6} cy={5} />
    </svg>
    {label(k)}
  </li>
{/each}

<style>
  li {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  svg {
    overflow: visible;
  }
  .mark {
    stroke: var(--muted);
    stroke-width: 1.5;
    stroke-dasharray: 3 3;
  }
  .mark.kept {
    stroke: var(--q-excellent);
  }
  .mark.reverted {
    stroke: var(--q-poor);
  }
</style>
