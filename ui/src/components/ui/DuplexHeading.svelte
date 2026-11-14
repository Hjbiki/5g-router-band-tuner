<script lang="ts">
  // The heading over one kind of band (FDD, TDD or SDL): its colour swatch, its name, what it means,
  // and how many of it are selected.
  import { t } from '../../lib/i18n'
  import { formatNumber, isolate } from '../../lib/format'
  import { app } from '../../lib/store.svelte'
  import type { Duplex } from '../../lib/bands'
  import InfoTip from './InfoTip.svelte'

  let {
    duplex,
    count,
    total,
    level = 'h3',
  }: { duplex: Duplex; count?: number; total?: number; level?: 'h3' | 'h4' } = $props()
  const digits = $derived(app.settings?.digits ?? 'western')
  const name = $derived(t(`bands.group.${duplex}`))
  // The help button names only the kind ("What is FDD?"), not the whole group title.
  const KIND: Record<Duplex, string> = { fdd: 'FDD', tdd: 'TDD', sdl: 'SDL' }
</script>

<div class="duplex-head duplex-{duplex}" data-testid="duplex-{duplex}">
  <span class="swatch" aria-hidden="true"></span>
  <svelte:element this={level} class="title" class:small={level === 'h4'}>{name}</svelte:element>
  <InfoTip label={t('metrics.whatIs', { name: KIND[duplex] })} text={t(`bands.help.${duplex}`)} />
  {#if count !== undefined && total !== undefined}
    <span class="count"
      >{t('bands.selectedCount', {
        n: isolate(formatNumber(count, { digits })),
        total: isolate(formatNumber(total, { digits })),
      })}</span
    >
  {/if}
</div>

<style>
  .duplex-head {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 2px 6px;
  }
  .swatch {
    flex: none;
    inline-size: 10px;
    block-size: 10px;
    border-radius: 3px;
    background: var(--duplex);
  }
  .title {
    margin: 0;
    font-family: var(--font-ui);
    font-size: var(--fs-body);
    font-weight: 600;
  }
  .title.small {
    color: var(--muted);
    font-size: 13px;
    font-weight: 500;
  }
  .count {
    margin-inline-start: auto;
    color: var(--faint);
    font-size: var(--fs-small);
  }
</style>
