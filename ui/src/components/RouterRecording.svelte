<script lang="ts">
  // Records what the router's own web page asks the router, for a developer to add the model
  // from. The page opens in a window of its own that can reach nothing but the router. Nothing
  // leaves the PC: the cleaned file is shown here, and the user saves and attaches it themselves.
  import { onMount } from 'svelte'
  import CircleDot from '@lucide/svelte/icons/circle-dot'
  import ExternalLink from '@lucide/svelte/icons/external-link'
  import FileDown from '@lucide/svelte/icons/file-down'
  import Square from '@lucide/svelte/icons/square'
  import { api } from '../lib/backend'
  import { app, pushToast } from '../lib/store.svelte'
  import { t } from '../lib/i18n'
  import { errorText } from '../lib/errors'
  import { formatNumber } from '../lib/format'
  import { ensureRecording, loadPreview, recording, takeCaptureStatus } from '../lib/recording.svelte'
  import { routerProbe } from '../lib/routerProbe.svelte'
  import type { CaptureRefusal } from '../lib/bindings/CaptureRefusal'
  import InfoTip from './ui/InfoTip.svelte'

  onMount(() => {
    void ensureRecording()
    recording.places += 1
    return () => {
      recording.places -= 1
    }
  })

  const digits = $derived(app.settings?.digits ?? 'western')
  const phase = $derived(recording.status.phase)
  const preview = $derived(recording.preview)

  let busy = $state(false)
  let error: string | null = $state(null)
  let refused: CaptureRefusal | null = $state(null)
  let includeCode = $state(false)
  let saved = $state(false)

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

  /** The backend's word on where the recording stands, after something this window asked for. */
  const refresh = async () => takeCaptureStatus(await api.captureStatus())

  const begin = () =>
    act(async () => {
      // What the window itself says: its title and its Stop item, before and after enough is
      // recorded, and the box of steps it opens with.
      refused = await api.captureStart({
        title: t('recording.windowTitle'),
        stop: t('recording.windowStop'),
        doneTitle: t('recording.windowDoneTitle'),
        doneStop: t('recording.windowDoneStop'),
        steps: t('recording.windowSteps'),
        ok: t('recording.ok'),
      })
      if (refused) return
      saved = false
      await refresh()
    })
  const stop = () =>
    act(async () => {
      includeCode = false
      const shown = await api.captureStop()
      await refresh()
      recording.preview = shown
    })
  const rebuild = () =>
    act(async () => {
      recording.preview = await api.capturePreview(includeCode)
    })
  const save = () =>
    act(async () => {
      if (!(await api.captureSave())) return
      saved = true
      pushToast(t('recording.saved'))
      await refresh()
    })
  const discard = () =>
    act(async () => {
      await api.captureDiscard()
      await refresh()
    })
  const openIssue = () => api.openRouterIssue().catch((e: unknown) => (error = errorText(e)))

  // The count moves while the window is open; the backend only announces the big steps.
  $effect(() => {
    if (phase !== 'recording') return
    const id = setInterval(() => {
      void api
        .captureStatus()
        .then((s) => takeCaptureStatus(s))
        .catch(() => {})
    }, 1000)
    return () => clearInterval(id)
  })

  // A recording stopped elsewhere (from its own window, or before this page was opened) still
  // needs its review.
  $effect(() => {
    if (phase !== 'stopped' || preview || busy || error) return
    void act(async () => {
      includeCode = false
      recording.preview = await loadPreview()
    })
  })

  // The review is what the user has to look at next, wherever on the page they were.
  let section: HTMLElement | undefined = $state()
  let broughtUp = false
  $effect(() => {
    if (phase !== 'stopped') broughtUp = false
    else if (preview && !broughtUp) {
      broughtUp = true
      section?.scrollIntoView?.({ block: 'start' })
    }
  })

  /** Whole kilobytes or megabytes: enough to know whether the file is small or large. */
  function size(bytes: number): string {
    const mb = bytes / (1024 * 1024)
    return mb >= 1
      ? `${formatNumber(Math.round(mb), { digits })} MB`
      : `${formatNumber(Math.max(1, Math.round(bytes / 1024)), { digits })} KB`
  }
  const count = (n: number) => formatNumber(n, { digits })
</script>

<section class="rec" aria-labelledby="recording-title" data-testid="router-recording" bind:this={section}>
  <h3 id="recording-title">
    {t('recording.title')}
    <InfoTip label={t('metrics.whatIs', { name: t('recording.title') })} text={t('recording.help')} />
  </h3>

  {#if phase === 'idle'}
    <p class="k">{t('recording.body')}</p>
    {#if saved && routerProbe.view?.support === 'unknown'}
      <!-- The form is the New router one, so it is offered only for a router the app can't work
           with yet. -->
      <p role="status">{t('recording.savedNext')}</p>
      <div class="row">
        <button type="button" class="primary" onclick={openIssue}>
          <ExternalLink size={16} aria-hidden="true" />{t('unsupported.openIssue')}
        </button>
      </div>
    {:else if saved}
      <p role="status">{t('recording.saved')}</p>
    {/if}
    <!-- Three steps, and one press. The cautions are behind the tip above. -->
    <ol>
      <li>{t('recording.step1')}</li>
      <li>{t('recording.step2')}</li>
      <li>{t('recording.step3')}</li>
    </ol>
    <div class="row">
      <button type="button" class:primary={!saved} disabled={busy} onclick={begin}>
        <CircleDot size={16} aria-hidden="true" />{t('recording.start')}
      </button>
    </div>
    {#if refused}<p class="err" role="alert">{t(`recording.refused.${refused}`)}</p>{/if}
  {:else if phase === 'recording'}
    <p role="status"><span class="dot" aria-hidden="true"></span>{t('recording.running', { n: count(recording.status.requests) })}</p>
    {#if recording.status.limit}<p class="k" role="status">{t(`recording.limit.${recording.status.limit}`)}</p>{/if}
    {#if recording.status.enough}<p role="status">{t('recording.ready')}</p>{/if}
    <div class="row">
      <button type="button" class="primary" disabled={busy} onclick={stop}>
        <Square size={16} aria-hidden="true" />{t('recording.stop')}
      </button>
      <button type="button" disabled={busy} onclick={discard}>{t('recording.discard')}</button>
    </div>
  {:else}
    <strong id="recording-review">{t('recording.reviewTitle')}</strong>
    {#if preview && preview.problems.length}
      <p class="err" role="alert">{t('recording.blocked', { what: preview.problems.join(', ') })}</p>
    {:else if preview}
      <p class="k">{t('recording.reviewBody')}</p>
      <!-- What a person can actually check: the file is far too long to read. Text only. -->
      {#if preview.readable.length}
        <p id="recording-readable">{t('recording.readable', { n: count(preview.readableTotal) })}</p>
        <ul class="words" class:long={preview.readable.length > 60} aria-labelledby="recording-readable">
          {#each preview.readable as value, i (i)}<li><bdi dir="ltr">{value}</bdi></li>{/each}
        </ul>
        {#if preview.readableTotal > preview.readable.length}
          <p class="k">
            {t('recording.readableMore', { n: count(preview.readableTotal - preview.readable.length) })}
          </p>
        {/if}
      {:else}
        <p>{t('recording.readableNone')}</p>
      {/if}
      <p class="k">{t('recording.readableNote')}</p>
      <p class="k">
        <span>{t('recording.requests', { n: count(preview.requests) })}</span>
        <span class="sep" aria-hidden="true">·</span>
        <bdi dir="ltr">{size(preview.bytes)}</bdi>
      </p>
      {#if preview.limit}<p class="k">{t(`recording.limit.${preview.limit}`)}</p>{/if}
      <textarea dir="ltr" readonly rows="8" aria-labelledby="recording-review" value={preview.text}></textarea>
      {#if preview.truncated}<p class="k">{t('recording.truncated')}</p>{/if}
      <label class="code">
        <input type="checkbox" bind:checked={includeCode} disabled={busy} onchange={rebuild} />
        <span>{t('recording.includeCode')}</span>
      </label>
      <p class="k">{t('recording.includeCodeHelp')}</p>
    {/if}
    <div class="row">
      {#if preview && !preview.problems.length}
        <button type="button" class="primary" disabled={busy} onclick={save}>
          <FileDown size={16} aria-hidden="true" />{t('recording.save')}
        </button>
      {/if}
      <button type="button" disabled={busy} onclick={discard}>{t('recording.discard')}</button>
    </div>
  {/if}
  {#if error}<p class="err" role="alert">{error}</p>{/if}
</section>

<style>
  .rec {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  h3 {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 0;
    font-size: var(--fs-body);
    font-weight: 600;
  }
  p {
    margin: 0;
  }
  .words {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin: 0;
    padding: 0;
    list-style: none;
  }
  /* The list is there to be read, so it is shown whole. Only a very long one gets a box. */
  .words.long {
    max-block-size: 420px;
    overflow-y: auto;
  }
  .words li {
    padding: 2px 8px;
    border: 1px solid var(--field-border);
    border-radius: 6px;
    font-family: var(--font-mono, ui-monospace, monospace);
    font-size: var(--fs-small, 13px);
    overflow-wrap: anywhere;
  }
  .k {
    color: var(--muted);
    font-size: var(--fs-small);
  }
  .sep {
    margin-inline: 6px;
  }
  strong {
    font-weight: 600;
  }
  ol {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin: 0;
    padding-inline-start: 22px;
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
  .dot {
    display: inline-block;
    inline-size: 8px;
    block-size: 8px;
    margin-inline-end: 8px;
    border-radius: 50%;
    background: var(--q-poor);
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
  .code {
    display: flex;
    align-items: center;
    gap: 8px;
    cursor: pointer;
  }
  .code input {
    accent-color: var(--accent);
  }
  .err {
    color: var(--q-poor);
  }
</style>
