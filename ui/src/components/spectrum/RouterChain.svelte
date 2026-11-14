<script lang="ts">
  // The Router page's picture in the Spectrum design: this PC, the router and the internet in a
  // row, with the state of each link written on it. It only shows what the snapshot already has.
  import { app } from '../../lib/store.svelte'
  import { t } from '../../lib/i18n'
  import { credentials } from '../../lib/credentials.svelte'
  import { loginShort, loginState, loginTone } from '../../lib/login'
  import { networkTypeLabel } from '../../lib/format'
  import { rangeText, wanKindShortKey } from '../../lib/network'

  const snap = $derived(app.snapshot)
  const lstate = $derived(loginState(snap?.session ?? null, credentials.status))
  const loggedIn = $derived(lstate.kind === 'connected')
  const inService = $derived(loggedIn && (snap?.inService ?? false))
  const network = $derived(inService ? networkTypeLabel(snap?.status?.networkType, snap?.signal) : null)
  const wan = $derived(loggedIn ? (snap?.wan ?? null) : null)
</script>

<section class="chain" aria-label={t('spectrum.chain.label')} data-testid="router-chain">
  <div class="node">
    <svg width="48" height="48" viewBox="0 0 48 48" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"
      ><rect x="6" y="9" width="36" height="24" rx="3"></rect><path d="M18 41 h12 M24 33 v8"></path></svg
    >
    <span class="name">{t('spectrum.chain.pc')}</span>
    <span class="sub">{t('app.title')}</span>
  </div>

  <div class="link">
    <span class="state {loginTone(lstate)}">{t(loginShort(lstate))}</span>
    <div class="bar" class:up={loggedIn}></div>
  </div>

  <div class="node">
    <svg width="48" height="48" viewBox="0 0 48 48" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"
      ><rect x="6" y="26" width="36" height="14" rx="3"></rect><path d="M14 33 h0.1 M20 33 h0.1 M34 26 V10"></path><path d="M28 12 a8 8 0 0 1 12 0"></path></svg
    >
    <span class="name"><bdi dir="ltr">{snap?.device?.model ?? t('shell.router')}</bdi></span>
    <span class="sub"><bdi dir="ltr">{app.settings?.host ?? ''}</bdi></span>
  </div>

  <div class="link">
    <span class="state" class:ok={inService} class:bad={loggedIn && !inService}>
      {#if network}<bdi>{network}</bdi>{:else if loggedIn}{t('spectrum.chain.noService')}{:else}—{/if}
    </span>
    <div class="bar" class:up={inService}></div>
  </div>

  <div class="node">
    <svg width="48" height="48" viewBox="0 0 48 48" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"
      ><circle cx="24" cy="24" r="17"></circle><path d="M7 24 h34 M24 7 c-7 6 -7 28 0 34 M24 7 c7 6 7 28 0 34"></path></svg
    >
    <span class="name">{t('spectrum.chain.internet')}</span>
    <span class="sub">
      {#if wan?.range}<bdi dir="ltr">{rangeText(wan.range)}</bdi>{#if wan.kind} · {t(wanKindShortKey(wan.kind))}{/if}{:else}—{/if}
    </span>
  </div>
</section>

<style>
  .chain {
    display: grid;
    grid-template-columns: auto minmax(60px, 1fr) auto minmax(60px, 1fr) auto;
    align-items: center;
    gap: 20px;
    background: var(--surface);
    border-radius: var(--radius-card);
    padding-block: var(--card-pad-block);
    padding-inline: 40px;
  }
  .node {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 4px;
    text-align: center;
    min-inline-size: 110px;
  }
  .name {
    font-weight: 600;
  }
  .sub,
  .state {
    color: var(--muted);
    font-size: var(--fs-small);
  }
  .link {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
  }
  .state {
    font-weight: 500;
  }
  .state.ok {
    color: var(--q-excellent);
  }
  .state.warn {
    color: var(--q-fair);
  }
  .state.bad {
    color: var(--q-poor);
  }
  /* A link that is down is a thin dashed line; one that is up is a solid bar in the accent. */
  .bar {
    inline-size: 100%;
    block-size: 0;
    border-block-start: 2px dashed var(--control);
  }
  .bar.up {
    block-size: 8px;
    border: 0;
    border-radius: 4px;
    background: var(--accent);
  }
  @media (max-width: 720px) {
    .chain {
      grid-template-columns: minmax(0, 1fr);
      padding-inline: var(--card-pad-inline);
    }
  }
</style>
