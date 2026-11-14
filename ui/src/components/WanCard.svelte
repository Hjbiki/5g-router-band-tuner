<script lang="ts">
  import Globe from '@lucide/svelte/icons/globe'
  import Lock from '@lucide/svelte/icons/lock'
  import LockOpen from '@lucide/svelte/icons/lock-open'
  import RefreshCw from '@lucide/svelte/icons/refresh-cw'
  import Undo2 from '@lucide/svelte/icons/undo-2'
  import { api } from '../lib/backend'
  import { app, pushToast } from '../lib/store.svelte'
  import { t, type MessageKey } from '../lib/i18n'
  import { errorText } from '../lib/errors'
  import { isolate } from '../lib/format'
  import { changeOpen, sameConfig } from '../lib/bands'
  import { rangeText, wanKindKey } from '../lib/network'
  import { ipRefreshActive, openIpRefresh, preferredMethod } from '../lib/ipRefresh.svelte'
  import Fact from './ui/Fact.svelte'

  const snap = $derived(app.snapshot)
  const loggedIn = $derived(snap?.session.kind === 'loggedIn')
  const wan = $derived(loggedIn ? (snap?.wan ?? null) : null)
  // Only for this view, and forgotten when the page closes: never saved.
  let showFull = $state(false)
  // Refresh IP shows when the driver has a way to get a new address (`ipMethods`).
  const method = $derived(loggedIn ? preferredMethod(snap) : null)
  const canRefresh = $derived(method !== null)

  // How the last band switch for a new IP ended, with its ranges, while nothing else is open.
  const reattach = $derived(snap?.ipReattach ?? null)
  const reattachLast = $derived(loggedIn && reattach && !reattach.active ? (reattach.last ?? null) : null)
  // The router may not have ended on the bands from before: offer them back (never by itself).
  const restoreTo = $derived.by(() => {
    const r = reattachLast?.restore
    if (!r) return null
    return snap?.config && sameConfig(snap.config, r) ? null : r
  })
  let restoring = $state(false)
  async function restore() {
    if (!restoreTo || restoring || changeOpen(snap?.safeApply)) return
    restoring = true
    try {
      await api.applyConfig(restoreTo)
    } catch (e) {
      pushToast(errorText(e), 'error')
    } finally {
      restoring = false
    }
  }
</script>

<section class="card" aria-labelledby="wan-title">
  <h2 id="wan-title"><Globe size={18} aria-hidden="true" />{t('network.wan.title')}</h2>
  {#if !wan}
    <p class="note" data-testid="wan-unknown">{loggedIn ? t('network.wan.unknown') : t('network.wan.loggedOut')}</p>
  {:else}
    <dl>
      <Fact label={t('network.wan.address')} help={t('network.wan.addressHelp')}>
        {#if wan.range}
          <span class="address">
            <bdi dir="ltr" class="num" data-testid="wan-address">{showFull && wan.ipv4 ? wan.ipv4 : rangeText(wan.range)}</bdi>
            <button type="button" class="link" aria-pressed={showFull} onclick={() => (showFull = !showFull)}>
              {showFull ? t('network.wan.hideFull') : t('network.wan.showFull')}
            </button>
          </span>
        {:else}
          <span data-testid="wan-address">{t('network.wan.none')}</span>
        {/if}
      </Fact>
      <Fact label={t('network.wan.type')} help={t('network.wan.typeHelp')}>
        {#if wan.kind}
          <span class="chip" data-testid="wan-kind">
            {#if wan.kind === 'public'}<LockOpen size={14} aria-hidden="true" />{:else}<Lock size={14} aria-hidden="true" />{/if}
            {t(wanKindKey(wan.kind))}
          </span>
        {:else}—{/if}
      </Fact>
      <Fact label={t('network.wan.ipv6')} term="IPv6" help={t('network.wan.ipv6Help')}>
        <span data-testid="wan-ipv6">{wan.ipv6 ? t('network.wan.yes') : t('network.wan.no')}</span>
      </Fact>
    </dl>
  {/if}
  {#if canRefresh}
    <div class="refresh">
      <button
        type="button"
        class="secondary"
        disabled={ipRefreshActive(snap)}
        onclick={() => openIpRefresh()}
        data-testid="wan-refresh"><RefreshCw size={16} aria-hidden="true" />{t('ipRefresh.open')}</button
      >
      <p class="note">{t(method === 'networkSearch' ? 'ipRefresh.openHintSearch' : 'ipRefresh.openHint')}</p>
    </div>
  {/if}
  {#if reattachLast}
    <div class="reattach" data-testid="wan-reattach-last">
      <p class="note">
        {t('ipReattach.lastLine', { outcome: t(`ipReattach.outcome.${reattachLast.outcome}` as MessageKey) })}
        {#if reattachLast.from && reattachLast.to}
          {t('ipReattach.ranges', { from: isolate(rangeText(reattachLast.from)), to: isolate(rangeText(reattachLast.to)) })}
        {/if}
      </p>
      {#if restoreTo}
        <button
          type="button"
          class="secondary"
          disabled={restoring || changeOpen(snap?.safeApply)}
          onclick={restore}
          data-testid="wan-reattach-restore"
          ><Undo2 size={16} aria-hidden="true" />{restoring
            ? t('ipRefresh.banner.restoring')
            : t('ipRefresh.banner.restoreBands')}</button
        >
      {/if}
    </div>
  {/if}
</section>

<style>
  .card {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius-card);
    padding-block: var(--card-pad-block);
    padding-inline: var(--card-pad-inline);
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-inline-size: 0;
  }
  dl {
    margin: 0;
  }
  .note {
    margin: 0;
    color: var(--muted);
  }
  .address {
    display: inline-flex;
    flex-wrap: wrap;
    align-items: baseline;
    justify-content: flex-end;
    gap: 4px 10px;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding-block: 2px;
    padding-inline: 8px;
    border: 1px solid var(--border);
    border-radius: var(--radius-control);
    background: var(--raised);
  }
  .link {
    all: unset;
    color: var(--accent);
    font-weight: 500;
    text-decoration: underline;
    cursor: pointer;
  }
  .refresh {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px 12px;
    margin-block-start: 4px;
  }
  .refresh button {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    min-block-size: var(--control-height);
    padding-inline: 14px;
    background: var(--secondary);
    border-color: var(--secondary);
    font-weight: 500;
  }
  .reattach {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px 12px;
  }
  .reattach .note {
    flex: 1 1 220px;
    font-size: var(--fs-small);
  }
  .reattach button {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    min-block-size: var(--control-height);
    padding-inline: 14px;
    background: var(--secondary);
    border-color: var(--secondary);
    font-weight: 500;
  }
  .refresh .note {
    flex: 1 1 220px;
    font-size: var(--fs-small);
  }
  .link:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
</style>
