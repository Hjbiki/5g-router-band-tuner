<script lang="ts">
  import { t } from '../../lib/i18n'

  // Phone-style bars from the router's own 0 to 5 score, coloured by how many are lit. The word
  // goes to screen readers and the hover title, so the colour is never the only signal.
  let { value, size = 18 }: { value: number | null | undefined; size?: number } = $props()

  const known = $derived(typeof value === 'number' && Number.isFinite(value))
  const lit = $derived(known ? Math.max(0, Math.min(5, Math.round(value as number))) : 0)
  const tone = $derived(lit >= 5 ? 'excellent' : lit === 4 ? 'good' : lit === 3 ? 'fair' : 'poor')
  const label = $derived(known ? t('metrics.barsOf', { n: lit }) : t('metrics.barsUnknown'))
</script>

<span class="bars" role="img" aria-label={label} title={label} style:--h="{size}px" style:--q="var(--q-{tone})">
  {#each [1, 2, 3, 4, 5] as i (i)}
    <i class:on={i <= lit} style:block-size="{Math.round(size * (0.28 + i * 0.144))}px"></i>
  {/each}
</span>

<style>
  /* Always rising to the right, also in RTL, like a phone's status bar. */
  .bars {
    direction: ltr;
    display: inline-flex;
    align-items: flex-end;
    gap: 2px;
    block-size: var(--h);
    flex: none;
  }
  i {
    display: block;
    inline-size: 4px;
    border-radius: 1.5px;
    background: var(--q-track);
  }
  i.on {
    background: var(--q);
  }
</style>
