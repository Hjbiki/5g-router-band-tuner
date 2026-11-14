<script lang="ts">
  // The operator's name with its logo, or a neutral icon without one. A logo may carry the
  // operator's own colours; nothing else in the app takes them.
  import RadioTower from '@lucide/svelte/icons/radio-tower'
  import { t } from '../../lib/i18n'
  import { identifyOperator, type OperatorInput } from '../../lib/operators'

  let { operator }: { operator: OperatorInput } = $props()
  const who = $derived(identifyOperator(operator))
</script>

<span class="badge" data-testid="operator-badge">
  {#if who.logo}
    <span class="tile {who.tile}"><img src={who.logo} alt="" /></span>
  {:else}
    <RadioTower size={14} aria-hidden="true" />
  {/if}
  <bdi>{who.name}</bdi>
  {#if who.via}<span class="via">{t('operator.via', { host: who.via })}</span>{/if}
</span>

<style>
  .badge {
    display: inline-flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    max-inline-size: 100%;
    padding-block: 2px;
    padding-inline: 6px;
    border: 1px solid var(--border);
    border-radius: var(--radius-chip);
    color: var(--text);
    line-height: 1.3;
    overflow-wrap: anywhere;
  }
  .badge :global(svg) {
    flex: none;
    color: var(--muted);
  }
  .via {
    color: var(--muted);
    font-size: var(--fs-small);
  }
  /* A small tile of its own, so a logo keeps its colours on any palette and theme. */
  .tile {
    display: inline-flex;
    align-items: center;
    flex: none;
    block-size: 20px;
    padding-inline: 4px;
    border-radius: 3px;
  }
  .tile.light {
    background: #ffffff;
    box-shadow: inset 0 0 0 1px rgb(0 0 0 / 0.08);
  }
  .tile.dark {
    background: #1f1f23;
  }
  img {
    display: block;
    block-size: 14px;
    inline-size: auto;
    max-inline-size: 56px;
  }
</style>
