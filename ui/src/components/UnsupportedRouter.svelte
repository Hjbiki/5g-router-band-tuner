<script lang="ts">
  // Shown to anyone on a router the app can't work with yet: what it found, and how to report it.
  // Nothing leaves the PC until the user saves the file and attaches it to an issue themselves.
  import { onMount } from 'svelte'
  import CircleHelp from '@lucide/svelte/icons/circle-help'
  import ExternalLink from '@lucide/svelte/icons/external-link'
  import FileDown from '@lucide/svelte/icons/file-down'
  import X from '@lucide/svelte/icons/x'
  import { api } from '../lib/backend'
  import { pushToast } from '../lib/store.svelte'
  import { t } from '../lib/i18n'
  import { errorText } from '../lib/errors'
  import { ensureRouterProbe, routerProbe } from '../lib/routerProbe.svelte'

  onMount(() => {
    void ensureRouterProbe()
  })

  const view = $derived(routerProbe.view)
  let dismissed = $state(false)
  const show = $derived(view?.support === 'unknown' && !dismissed)
  const name = $derived([view?.vendor, view?.model].filter(Boolean).join(' '))

  // The report, once built; shown in full before it can be saved.
  let text: string | null = $state(null)
  let saved = $state(false)
  let busy = $state(false)
  let error: string | null = $state(null)

  async function act(work: () => Promise<void>) {
    busy = true
    error = null
    try {
      await work()
    } catch (e) {
      error = errorText(e)
    } finally {
      busy = false
    }
  }
  const prepare = () =>
    act(async () => {
      text = await api.prepareRouterReport()
    })
  const save = () =>
    act(async () => {
      if (await api.saveDiagnostics()) {
        saved = true
        pushToast(t('unsupported.saved'))
      }
    })
  const openIssue = () => api.openRouterIssue().catch((e: unknown) => (error = errorText(e)))
</script>

{#if show && view}
  <section class="notice" aria-labelledby="unsupported-title" data-testid="unsupported-router">
    <div class="top">
      <h2 id="unsupported-title"><CircleHelp size={18} aria-hidden="true" />{t('unsupported.title')}</h2>
      <button type="button" class="close" aria-label={t('app.dismiss')} onclick={() => (dismissed = true)}>
        <X size={16} aria-hidden="true" />
      </button>
    </div>

    <p>
      {#if name}{t('unsupported.found', { name: '' })}<bdi dir="ltr"><strong>{name}</strong></bdi>{:else}{t('unsupported.foundNothing')}{/if}
      {#if view.hardware}<span class="k">{t('unsupported.hardware')}: <bdi dir="ltr">{view.hardware}</bdi></span>{/if}
      {#if view.firmware}<span class="k">{t('unsupported.firmware')}: <bdi dir="ltr">{view.firmware}</bdi></span>{/if}
    </p>
    <p>{t('unsupported.body')}</p>
    <p class="k">{t('unsupported.noPassword')}</p>

    {#if text === null}
      <div class="row">
        <button type="button" class="primary" disabled={busy} onclick={prepare}>
          <FileDown size={16} aria-hidden="true" />{t('unsupported.help')}
        </button>
      </div>
    {:else}
      <ol>
        <li>
          <strong id="unsupported-review">{t('unsupported.step1')}</strong>
          <p class="k">{t('unsupported.step1Hint')}</p>
          <textarea dir="ltr" readonly rows="10" aria-labelledby="unsupported-review" value={text}></textarea>
        </li>
        <li>
          <strong>{t('unsupported.step2')}</strong>
          <div class="row">
            <button type="button" class:primary={!saved} disabled={busy} onclick={save}>
              <FileDown size={16} aria-hidden="true" />{t('diagnostics.save')}
            </button>
          </div>
        </li>
        <li>
          <strong>{t('unsupported.step3')}</strong>
          <p class="k">{t('unsupported.step3Hint')}</p>
          <div class="row">
            <button type="button" class:primary={saved} onclick={openIssue}>
              <ExternalLink size={16} aria-hidden="true" />{t('unsupported.openIssue')}
            </button>
          </div>
        </li>
      </ol>
    {/if}
    {#if error}<p class="err" role="alert">{error}</p>{/if}
  </section>
{/if}

<style>
  .notice {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding-block: var(--card-pad-block);
    padding-inline: var(--card-pad-inline);
    border: 1px solid var(--border);
    border-radius: var(--radius-card);
    background: var(--surface);
  }
  .top {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
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
    flex: none;
  }
  .close {
    all: unset;
    display: inline-grid;
    place-items: center;
    inline-size: 26px;
    block-size: 26px;
    border-radius: 5px;
    color: var(--muted);
    cursor: pointer;
  }
  .close:hover {
    background: var(--active);
    color: var(--text);
  }
  .close:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
  p {
    margin: 0;
  }
  .k {
    color: var(--muted);
    font-size: var(--fs-small);
  }
  p .k {
    margin-inline-start: 8px;
  }
  strong {
    font-weight: 600;
  }
  ol {
    display: flex;
    flex-direction: column;
    gap: 12px;
    margin: 0;
    padding-inline-start: 20px;
  }
  li {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  textarea {
    inline-size: 100%;
    box-sizing: border-box;
    resize: vertical;
    margin: 0;
    padding: 10px;
    font-family: ui-monospace, 'Cascadia Mono', Consolas, monospace;
    font-size: var(--fs-small);
    color: var(--text);
    background: var(--raised);
    border: 1px solid var(--border);
    border-radius: var(--radius-control);
    white-space: pre;
    overflow: auto;
  }
  .row {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  .row button {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    min-block-size: var(--control-height);
    font-weight: 500;
  }
  .err {
    color: var(--q-poor);
  }
</style>
