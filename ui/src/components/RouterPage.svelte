<script lang="ts">
  // The Router page: logging in, the internet address and Refresh IP, the device with restart
  // and factory reset, APN profiles, its page scripts and the read-only privacy check.
  // App preferences stay in Settings.
  import Router from '@lucide/svelte/icons/router'
  import Power from '@lucide/svelte/icons/power'
  import { app } from '../lib/store.svelte'
  import { t } from '../lib/i18n'
  import RebootDialog from './RebootDialog.svelte'
  import PageScriptsExport from './PageScriptsExport.svelte'
  import FactoryReset from './FactoryReset.svelte'
  import LoginPanel from './LoginPanel.svelte'
  import WanCard from './WanCard.svelte'
  import ApnProfiles from './ApnProfiles.svelte'
  import PrivacyCheck from './PrivacyCheck.svelte'
  import KeyRound from '@lucide/svelte/icons/key-round'
  import RouterChain from './spectrum/RouterChain.svelte'

  const device = $derived(app.snapshot?.device ?? null)
</script>

<section class="router" aria-labelledby="router-title">
  <header>
    <h1 id="router-title">{t('router.title')}</h1>
    <p class="lede">{t('router.intro')}</p>
  </header>

  {#if app.settings?.design === 'spectrum'}<div class="chain-row"><RouterChain /></div>{/if}

  <div class="grid">
    <div class="card">
      <h2><KeyRound size={18} aria-hidden="true" />{t('router.connection')}</h2>
      <LoginPanel variant="settings" />
    </div>

    <WanCard />

    <div class="card">
      <h2><Router size={18} aria-hidden="true" />{t('router.device')}</h2>
      {#if device}
        <dl>
          <dt>{t('about.model')}</dt>
          <dd><bdi dir="ltr">{device.model}</bdi></dd>
          <dt>{t('about.firmware')}</dt>
          <dd><bdi dir="ltr" class="num">{device.firmware}</bdi></dd>
        </dl>
      {:else}
        <p class="muted">{t('about.notRead')}</p>
      {/if}
      <h3><Power size={16} aria-hidden="true" />{t('about.maintenance')}</h3>
      <p class="muted">{t('about.restartHint')}</p>
      <!-- Restart, and the factory reset beside it in red. Each opens its own warning first. -->
      <div class="maintenance" data-testid="maintenance">
        <RebootDialog />
        <FactoryReset />
      </div>
    </div>

    <div class="card">
      <PageScriptsExport />
    </div>
  </div>

  <ApnProfiles />

  <PrivacyCheck />
</section>

<style>
  .router {
    display: flex;
    flex-direction: column;
    gap: var(--gap);
  }
  h1 {
    font-size: var(--fs-title);
    line-height: 32px;
    margin: 0;
  }
  .lede {
    margin-block: 2px 0;
    color: var(--muted);
    max-inline-size: var(--measure);
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: var(--gap);
    align-items: start;
  }
  h3 {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-block: 10px 0;
    font-size: var(--fs-body);
  }
  .maintenance {
    display: flex;
    flex-wrap: wrap;
    align-items: flex-start;
    gap: 4px 10px;
  }
  @media (max-width: 860px) {
    .grid {
      grid-template-columns: minmax(0, 1fr);
    }
  }
  .card {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius-card);
    padding-block: var(--card-pad-block);
    padding-inline: var(--card-pad-inline);
    display: flex;
    flex-direction: column;
    gap: 6px;
    min-inline-size: 0;
  }
  p {
    margin: 0;
  }
  dl {
    display: grid;
    grid-template-columns: max-content minmax(0, 1fr);
    gap: 4px var(--gap);
    margin: 0;
  }
  dt {
    color: var(--muted);
  }
  /* A real firmware name is one long word (seen on the H122: 23 characters with no space). It
     wraps inside the card instead of pushing out of it and giving the page a sideways scrollbar. */
  dd {
    margin: 0;
    min-inline-size: 0;
    overflow-wrap: anywhere;
  }
  .muted {
    color: var(--muted);
  }

  /* Spectrum fills the width: three blocks across, then the page scripts beside the APN profiles. */
  @media (min-width: 1100px) {
    :global(:root[data-design='spectrum']) .router {
      display: grid;
      grid-template-columns: repeat(3, minmax(0, 1fr));
      align-items: stretch;
    }
    :global(:root[data-design='spectrum']) .grid {
      display: contents;
    }
    :global(:root[data-design='spectrum']) .router > header {
      grid-column: 1 / -1;
    }
    :global(:root[data-design='spectrum']) .router > :global(section.card) {
      grid-column: span 2;
    }
    /* The privacy check takes the whole row; its parts share it. */
    :global(:root[data-design='spectrum']) .router > :global(section.card.privacy) {
      grid-column: 1 / -1;
    }
  }

  .chain-row {
    grid-column: 1 / -1;
  }
</style>
