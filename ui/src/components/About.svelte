<script lang="ts">
  import { onMount } from 'svelte'
  import { getVersion } from '@tauri-apps/api/app'
  import ShieldCheck from '@lucide/svelte/icons/shield-check'
  import FileDown from '@lucide/svelte/icons/file-down'
  import pkg from '../../package.json'
  import { api } from '../lib/backend'
  import { t } from '../lib/i18n'
  import DiagnosticsExport from './DiagnosticsExport.svelte'
  import UpdateCard from './UpdateCard.svelte'

  let version = $state(pkg.version)
  onMount(() => {
    getVersion()
      .then((v) => (version = v))
      .catch(() => {})
  })
</script>

<section class="about" aria-labelledby="about-title">
  <h1 id="about-title">{t('about.title')}</h1>

  <div class="cards">
    <div class="card app">
      <svg class="mark" width="28" height="28" viewBox="0 0 22 22" aria-hidden="true">
        <rect x="1" y="13" width="4" height="8" rx="1"></rect>
        <rect x="7" y="9" width="4" height="12" rx="1"></rect>
        <rect class="hi" x="13" y="5" width="4" height="16" rx="1"></rect>
        <rect x="19" y="1" width="2" height="20" rx="1"></rect>
      </svg>
      <div class="lines">
        <p class="name">{t('app.title')}</p>
        <p class="muted"><bdi dir="ltr" class="num">{t('about.version', { version })}</bdi></p>
        <p>
          {t('about.developedBy')}
          <a
            href="https://creators.sa/hibiki"
            title={t('about.developerLinkHint')}
            onclick={(e) => {
              e.preventDefault()
              api.openDeveloperPage().catch(() => {})
            }}><bdi>{t('about.developerName')}</bdi></a
          >
        </p>
      </div>
    </div>

    <UpdateCard />

    <div class="card diagnostics">
      <h2><FileDown size={18} aria-hidden="true" />{t('diagnostics.title')}</h2>
      <DiagnosticsExport />
    </div>

    <div class="card wide">
      <h2><ShieldCheck size={18} aria-hidden="true" />{t('about.privacy')}</h2>
      <p>{t('about.localOnly')}</p>
      <p>{t('about.license')}</p>
      <p class="muted">{t('about.notAffiliated')}</p>
      <p class="muted">{t('about.disclaimer')}</p>
    </div>
  </div>

</section>

<style>
  .about {
    display: flex;
    flex-direction: column;
    gap: var(--gap);
  }
  h1 {
    font-size: var(--fs-title);
    line-height: 36px;
    margin: 0;
  }
  .cards {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: var(--gap);
    align-items: start;
  }
  .diagnostics {
    grid-column: 2;
    grid-row: 1 / span 2;
  }
  .wide {
    grid-column: 1 / -1;
  }
  @media (max-width: 860px) {
    .cards {
      grid-template-columns: minmax(0, 1fr);
    }
    .diagnostics {
      grid-column: auto;
      grid-row: auto;
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
  .card.app {
    flex-direction: row;
    align-items: flex-start;
    gap: 14px;
  }
  .lines {
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-inline-size: 0;
  }
  .mark {
    flex: none;
    fill: var(--text);
    margin-block-start: 2px;
  }
  .mark .hi {
    fill: var(--accent);
  }
  .name {
    font-family: var(--font-heading);
    font-weight: 700;
    font-size: var(--fs-section);
    line-height: 24px;
  }
  p {
    margin: 0;
  }
  a {
    color: var(--accent);
  }
  .muted {
    color: var(--muted);
  }

  /* Spectrum fills the width: the app and updates stacked, then diagnostics, then privacy. */
  @media (min-width: 1100px) {
    :global(:root[data-design='spectrum']) .cards {
      grid-template-columns: repeat(3, minmax(0, 1fr));
      align-items: stretch;
    }
    :global(:root[data-design='spectrum']) .diagnostics {
      grid-column: 2;
      grid-row: 1 / span 2;
    }
    :global(:root[data-design='spectrum']) .wide {
      grid-column: 3;
      grid-row: 1 / span 2;
    }
  }
</style>
