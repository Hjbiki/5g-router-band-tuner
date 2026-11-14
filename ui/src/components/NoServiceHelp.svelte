<script lang="ts">
  // No service on the Signal page: what the user can do about it, in the order to try. Every action
  // only opens another page; nothing here talks to the router.
  import LifeBuoy from '@lucide/svelte/icons/life-buoy'
  import { app, canTryBands, tryBands } from '../lib/store.svelte'
  import { t } from '../lib/i18n'
  import { isolate } from '../lib/format'
  import { bandsOf, supportedBands } from '../lib/bands'

  const snap = $derived(app.snapshot)
  const allowed = $derived(bandsOf(snap?.config))
  const supported = $derived(snap?.capabilities ? supportedBands(snap.capabilities.supportedLte) : [])
  /** The router is held to fewer bands than it supports, the usual cause after a band change. */
  const limited = $derived(allowed.length > 0 && allowed.length < supported.length)
  const canAllowAll = $derived(limited && Boolean(snap?.capabilities?.bandEditAllowed) && canTryBands())
</script>

<section class="help" aria-labelledby="no-service-help" data-testid="no-service-help">
  <h2 id="no-service-help"><LifeBuoy size={18} aria-hidden="true" />{t('dashboard.noService.title')}</h2>
  <ol>
    <li>{t('dashboard.noService.wait')}</li>
    {#if limited}
      <li>
        {t('dashboard.noService.limited', { bands: isolate(allowed.map((b) => `B${b}`).join(', ')) })}
        {#if canAllowAll}
          <span class="row">
            <button type="button" class="primary" data-testid="allow-all" onclick={() => snap?.capabilities && tryBands(snap.capabilities.supportedLte)}
              >{t('dashboard.noService.allowAll')}</button
            >
            <span class="muted">{t('dashboard.noService.allowAllHint')}</span>
          </span>
        {/if}
      </li>
    {/if}
    <li>
      {t('dashboard.noService.restart')}
      <span class="row">
        <button type="button" onclick={() => (app.tab = 'router')}>{t('dashboard.noService.openRouter')}</button>
      </span>
    </li>
  </ol>
</section>

<style>
  .help {
    padding-block: var(--card-pad-block);
    padding-inline: var(--card-pad-inline);
    border: 1px solid var(--border);
    border-radius: var(--radius-card);
    background: var(--surface);
  }
  h2 {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 0;
    font-size: var(--fs-section);
    font-weight: 600;
  }
  h2 :global(svg) {
    color: var(--accent);
  }
  ol {
    margin-block: 8px 0;
    margin-inline: 0;
    padding-inline-start: 22px;
    display: flex;
    flex-direction: column;
    gap: 10px;
    max-inline-size: var(--measure);
  }
  .row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px 10px;
    margin-block-start: 6px;
  }
  .muted {
    color: var(--muted);
    font-size: var(--fs-small);
  }
  button {
    block-size: var(--control-height);
    padding-inline: 14px;
    font-weight: 500;
  }
</style>
