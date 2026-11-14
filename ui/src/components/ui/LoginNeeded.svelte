<script lang="ts">
  // What a page shows in place of router data while nobody is logged in. The login itself is the
  // banner above every page and the Router page; this says why the page is empty and how to get there.
  import KeyRound from '@lucide/svelte/icons/key-round'
  import { app } from '../../lib/store.svelte'
  import { t, type MessageKey } from '../../lib/i18n'

  let { what }: { what: MessageKey } = $props()
</script>

<section class="need" data-testid="login-needed">
  <span class="icon" aria-hidden="true"><KeyRound size={20} /></span>
  <div class="text">
    <h2>{t('login.needed.title')}</h2>
    <p>{t(what)}</p>
  </div>
  <button type="button" class="primary" onclick={() => (app.tab = 'router')}>{t('login.needed.open')}</button>
</section>

<style>
  .need {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 12px 16px;
    padding-block: var(--card-pad-block);
    padding-inline: var(--card-pad-inline);
    border: 1px dashed var(--border);
    border-radius: var(--radius-card);
    background: var(--surface);
  }
  .icon {
    display: grid;
    place-items: center;
    inline-size: 40px;
    block-size: 40px;
    border-radius: 50%;
    background: var(--ground);
    color: var(--accent);
  }
  .text {
    flex: 1;
    min-inline-size: 220px;
  }
  h2 {
    margin: 0;
    font-size: var(--fs-section);
    font-weight: 600;
  }
  p {
    margin-block: 2px 0;
    margin-inline: 0;
    color: var(--muted);
  }
  button {
    block-size: var(--control-height);
    padding-inline: 14px;
    font-weight: 500;
  }
</style>
