<script lang="ts">
  import ArrowDown from '@lucide/svelte/icons/arrow-down'
  import ArrowUp from '@lucide/svelte/icons/arrow-up'
  import Activity from '@lucide/svelte/icons/activity'
  import Timer from '@lucide/svelte/icons/timer'
  import History from '@lucide/svelte/icons/history'
  import Settings2 from '@lucide/svelte/icons/settings-2'
  import Gauge from '@lucide/svelte/icons/gauge'
  import ExternalLink from '@lucide/svelte/icons/external-link'
  import { onMount } from 'svelte'
  import InfoTip from './ui/InfoTip.svelte'
  import SpeedHistoryTable from './SpeedHistoryTable.svelte'
  import { api } from '../lib/backend'
  import { scanActive } from '../lib/scan.svelte'
  import { app } from '../lib/store.svelte'
  import { changeOpen } from '../lib/bands'
  import { t } from '../lib/i18n'
  import { errorText } from '../lib/errors'
  import { formatNumber, isolate } from '../lib/format'
  import type { SpeedTestProvider } from '../lib/bindings/SpeedTestProvider'
  import type { SpeedTestSize } from '../lib/bindings/SpeedTestSize'
  import {
    STAGES,
    busyPing,
    cancelTest,
    diagnose,
    ensureSpeedTest,
    fallbackNote,
    invalidReason,
    notComparableKey,
    latLevel,
    mbps,
    ms,
    saveSpeedSettings,
    servingBands,
    servingDetail,
    speed,
    startTest,
    whenLabel,
    type Stage,
  } from '../lib/speedtest.svelte'

  onMount(() => {
    void ensureSpeedTest()
  })

  const digits = $derived(app.settings?.digits ?? 'western')
  const last = $derived(speed.history[0] ?? null)
  const spectrum = $derived(app.settings?.design === 'spectrum')
  /** Download against upload as two bars on one scale, for the Spectrum design. */
  const lanes = $derived.by(() => {
    if (speed.running || !last || !last.downloadMbps || !last.uploadMbps) return null
    const top = Math.max(last.downloadMbps, last.uploadMbps)
    return {
      down: Math.max(2, (last.downloadMbps / top) * 100),
      up: Math.max(2, (last.uploadMbps / top) * 100),
      times: Math.round(last.downloadMbps / last.uploadMbps),
    }
  })
  const diag = $derived(diagnose(last, digits))
  const cfg = $derived(app.settings?.speedTest ?? { provider: 'cloudflare', size: 'light', ooklaTermsAccepted: false })

  // A band change is open: the backend refuses, so the button says why up front.
  const scanning = $derived(scanActive())
  const bandBusy = $derived(changeOpen(app.snapshot?.safeApply) || scanning)

  // While running, the live values; otherwise the newest saved result.
  const values = $derived(speed.running ? speed.live : last)
  const busy = $derived(values ? busyPing(values) : { ms: null, during: null })
  const serving = $derived(speed.running ? speed.start?.serving : last?.serving)
  const opaque = $derived(speed.phase === 'installing' || speed.phase === 'external')

  const ORDER = { idlePing: 0, download: 1, upload: 2, done: 3 } as const
  function stageFraction(s: Stage): number {
    const p = speed.phase
    if (p === null || p === 'installing' || p === 'external') return 0
    const cur = ORDER[p]
    if (s === 'busy') {
      if (cur < 1) return 0
      if (cur === 1) return speed.fraction / 2
      if (cur === 2) return 0.5 + speed.fraction / 2
      return 1
    }
    const idx = ORDER[s]
    return idx < cur ? 1 : idx === cur ? speed.fraction : 0
  }

  function stageValue(s: Stage): { text: string; unit: string; level: ReturnType<typeof latLevel> } {
    const v = values
    const u = (k: 'ms' | 'mbps') => t(`speedtest.unit.${k}`)
    switch (s) {
      case 'idlePing':
        return { text: ms(v?.idlePingMs ?? null, digits), unit: u('ms'), level: null }
      case 'download':
        return { text: mbps(v?.downloadMbps ?? null, digits), unit: u('mbps'), level: null }
      case 'upload':
        return { text: mbps(v?.uploadMbps ?? null, digits), unit: u('mbps'), level: null }
      case 'busy':
        return { text: ms(busy.ms, digits), unit: u('ms'), level: latLevel(busy.ms) }
    }
  }

  const STAGE_ICONS = { idlePing: Timer, download: ArrowDown, upload: ArrowUp, busy: Activity } as const
  const busyHelpText = $derived(
    busy.during === 'upload'
      ? t('metrics.help.pingUpload')
      : busy.during === 'download'
        ? t('metrics.help.pingDownload')
        : t('speedtest.busyHelp'),
  )
  function stageHelp(s: Stage): string {
    if (s === 'busy') return busyHelpText
    return t(s === 'idlePing' ? 'metrics.help.idlePing' : s === 'download' ? 'metrics.help.download' : 'metrics.help.upload')
  }

  const statusLine = $derived.by(() => {
    const parts: string[] = []
    if (serving) {
      const detail = servingDetail(serving)
      parts.push(t('speedtest.onServing', { serving: isolate(servingBands(serving)) + (detail ? `, ${detail}` : '') }))
    }
    if (speed.running) {
      if (speed.start) parts.push(t(`speedtest.provider.${speed.start.provider}`))
      if (speed.bytesUsed > 0)
        parts.push(t('speedtest.used', { mb: formatNumber(speed.bytesUsed / 1e6, { digits, fraction: 0 }) }))
    } else if (last) {
      parts.push(last.server ?? t(`speedtest.provider.${last.provider}`))
      parts.push(whenLabel(last.atMs, Date.now(), digits))
    }
    return parts.join(' · ')
  })

  // --- Settings on this page ---
  let termsOpen = $state(false)
  let settingsError: string | null = $state(null)
  let saving = $state(false)

  async function save(patch: Parameters<typeof saveSpeedSettings>[0]) {
    saving = true
    settingsError = null
    try {
      await saveSpeedSettings(patch)
    } catch (e) {
      settingsError = t('speedtest.settings.saveFailed', { message: errorText(e) })
    } finally {
      saving = false
    }
  }

  function chooseProvider(p: SpeedTestProvider) {
    if (p === 'ookla' && !cfg.ooklaTermsAccepted) {
      // Nothing is saved or downloaded until the terms are accepted.
      termsOpen = true
      return
    }
    termsOpen = false
    if (p !== cfg.provider) void save({ provider: p })
  }

  async function acceptTerms() {
    await save({ provider: 'ookla', ooklaTermsAccepted: true })
    if (!settingsError) termsOpen = false
  }

  function chooseSize(s: SpeedTestSize) {
    if (s !== cfg.size) void save({ size: s })
  }

  function openDoc(doc: 'eula' | 'privacy') {
    api.openOoklaTerms(doc).catch((e) => (settingsError = errorText(e)))
  }
</script>

<section class="page" aria-labelledby="speedtest-title">
  <div class="head">
    <div class="titles">
      <h1 id="speedtest-title">{t('speedtest.title')}</h1>
      <p class="diag" data-testid="diagnosis"><b>{diag.headline}</b> <span class="muted">{diag.detail}</span></p>
    </div>
  </div>

  <section class="card" aria-labelledby="st-settings">
    <div class="topcard">
      <div class="cardhead col">
        <h2 id="st-settings" class="strong"><Settings2 size={18} aria-hidden="true" />{t('speedtest.settings.title')}</h2>
        <span class="k">{t('speedtest.settings.note')}</span>
      </div>
      {#if speed.running}
        <button type="button" class="secondary" disabled={scanning} onclick={cancelTest}>{t('speedtest.cancel')}</button>
      {:else}
        <button type="button" class="primary run" onclick={startTest} disabled={bandBusy}>
          <Gauge size={16} aria-hidden="true" />{last ? t('speedtest.runAgain') : t('speedtest.run')}
        </button>
      {/if}
    </div>
    {#if bandBusy && !speed.running}<p class="note">{t(scanning ? 'scan.blockedRunning' : 'speedtest.error.busy')}</p>{/if}
    <div class="settings">
      <fieldset disabled={speed.running || saving}>
        <legend>{t('speedtest.settings.provider')}</legend>
        <div class="seg">
          <label class:on={cfg.provider === 'cloudflare' && !termsOpen}>
            <input
              type="radio"
              name="st-provider"
              checked={cfg.provider === 'cloudflare' && !termsOpen}
              onchange={() => chooseProvider('cloudflare')}
            />{t('speedtest.settings.cloudflare')}
          </label>
          <label class:on={cfg.provider === 'ookla' || termsOpen}>
            <input
              type="radio"
              name="st-provider"
              checked={cfg.provider === 'ookla' || termsOpen}
              onchange={() => chooseProvider('ookla')}
            />{t('speedtest.settings.ookla')}
          </label>
        </div>
      </fieldset>
      <!-- Ookla's CLI has no data budget: it runs at full speed for its whole length. -->
      <fieldset
        disabled={speed.running || saving || cfg.provider === 'ookla'}
        aria-describedby={cfg.provider === 'ookla' || termsOpen ? 'st-size-ookla' : undefined}
      >
        <legend>{t('speedtest.settings.size')}</legend>
        <div class="seg">
          <label class:on={cfg.size === 'light'}>
            <input type="radio" name="st-size" checked={cfg.size === 'light'} onchange={() => chooseSize('light')} />
            {t('speedtest.settings.light')}
          </label>
          <label class:on={cfg.size === 'full'}>
            <input type="radio" name="st-size" checked={cfg.size === 'full'} onchange={() => chooseSize('full')} />
            {t('speedtest.settings.full')}
          </label>
        </div>
      </fieldset>
    </div>
    {#if cfg.provider === 'ookla' || termsOpen}
      <p id="st-size-ookla" class="note" data-testid="ookla-size">{t('speedtest.settings.ooklaSize')}</p>
    {/if}

    {#if termsOpen}
      <div class="terms" role="region" aria-labelledby="st-terms">
        <h3 id="st-terms">{t('speedtest.terms.title')}</h3>
        <ul>
          <li>{t('speedtest.terms.download')}</li>
          <li>{t('speedtest.terms.personal')}</li>
          <li>{t('speedtest.terms.data')}</li>
          <li>{t('speedtest.terms.dataUse')}</li>
        </ul>
        <div class="row">
          <button type="button" class="link" onclick={() => openDoc('eula')}>
            {t('speedtest.terms.eula')}<ExternalLink size={14} aria-hidden="true" />
          </button>
          <button type="button" class="link" onclick={() => openDoc('privacy')}>
            {t('speedtest.terms.privacy')}<ExternalLink size={14} aria-hidden="true" />
          </button>
        </div>
        <div class="row">
          <button type="button" class="primary" onclick={acceptTerms} disabled={saving}>{t('speedtest.terms.accept')}</button>
          <button type="button" class="secondary" onclick={() => (termsOpen = false)}>{t('speedtest.terms.decline')}</button>
        </div>
      </div>
    {:else if cfg.provider === 'ookla' && cfg.ooklaTermsAccepted}
      <p class="k">
        {t('speedtest.terms.accepted')}
        <button type="button" class="link" onclick={() => openDoc('eula')}>{t('speedtest.terms.eula')}</button>
        ·
        <button type="button" class="link" onclick={() => openDoc('privacy')}>{t('speedtest.terms.privacy')}</button>
      </p>
    {/if}
    {#if settingsError}<p class="err" role="alert">{settingsError}</p>{/if}
  </section>

  <section class="card" aria-label={speed.running ? t('speedtest.testingNow') : t('speedtest.lastResult')}>
    <!-- Every line here keeps its place, so the numbers don't move while a test runs. -->
    <div class="cardhead col">
      <span class="strong">
        {speed.running ? t('speedtest.testingNow') : last ? t('speedtest.lastResult') : t('speedtest.noResult')}
      </span>
      <span class="k status">{statusLine}</span>
    </div>
    <div class="stages">
      {#each STAGES as s (s)}
        {@const v = stageValue(s)}
        {@const StageIcon = STAGE_ICONS[s]}
        <div class="stage" class:lead={s === 'download'}>
          <span class="k label">
            <StageIcon size={15} aria-hidden="true" />
            {s === 'busy' && busy.during && !speed.running
              ? t(busy.during === 'upload' ? 'speedtest.pingWhileUploading' : 'speedtest.pingWhileDownloading')
              : t(`speedtest.phase.${s}`)}
            <InfoTip
              label={s === 'busy' ? t('speedtest.busyHelpLabel') : t('metrics.whatIs', { name: t(`speedtest.phase.${s}`) })}
              text={stageHelp(s)}
            />
          </span>
          <span class="figure num" data-testid={`stage-${s}`}>
            <bdi dir="ltr" class={v.level ? `lat-${v.level}` : ''}>{v.text}</bdi>
            <span class="k">{v.unit}{#if v.level && v.level !== 'low'}{' · '}{t(`speedtest.lat.${v.level}`)}{/if}</span>
          </span>
          {#if speed.running}
            {@const f = stageFraction(s)}
            <div
              class="bar"
              class:opaque
              role="progressbar"
              aria-label={t('speedtest.progressLabel', { stage: t(`speedtest.phase.${s}`) })}
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={opaque ? undefined : Math.round(f * 100)}
            >
              <i style:inline-size={opaque ? '100%' : `${f * 100}%`}></i>
            </div>
          {:else}
            <div class="bar resting" aria-hidden="true"></div>
          {/if}
        </div>
      {/each}
    </div>
    {#if spectrum && lanes}
      <div class="lanes" role="img" aria-label={t('spectrum.speedLanes')} data-testid="speed-lanes">
        <div class="lane"><span class="k">{t('speedtest.phase.download')}</span><i class="main" style:inline-size="{lanes.down}%"></i></div>
        <div class="lane"><span class="k">{t('speedtest.phase.upload')}</span><i style:inline-size="{lanes.up}%"></i></div>
      </div>
      {#if lanes.times >= 2}<p class="k">{t('spectrum.speedRatio', { n: isolate(formatNumber(lanes.times, { digits })) })}</p>{/if}
    {/if}
    {#if speed.running && speed.phase === 'installing'}
      <p class="note" role="status">{t('speedtest.installing')}</p>
    {:else if speed.running && speed.phase === 'external'}
      <p class="note" role="status">{t('speedtest.external')}</p>
    {/if}
    {#if speed.error}
      <div class="err" role="alert">
        <p>{speed.error}</p>
        {#if speed.errorCause}<p class="cause" dir="ltr">{t('speedtest.failCause', { message: speed.errorCause })}</p>{/if}
      </div>
    {/if}
    {#if speed.cancelled}<p class="note" role="status">{t('speedtest.cancelled')}</p>{/if}
    {#if !speed.running && last && invalidReason(last)}
      <p class="note" data-testid="last-not-comparable">{t(notComparableKey(last))}</p>
    {/if}
    {#if !speed.running && last && fallbackNote(last)}<p class="note">{fallbackNote(last)}</p>{/if}
  </section>

  <section class="card" aria-labelledby="st-history">
    <div class="cardhead">
      <h2 id="st-history" class="strong"><History size={18} aria-hidden="true" />{t('speedtest.history.title')}</h2>
      <span class="k">{t('speedtest.history.local')}</span>
    </div>
    {#if speed.history.length === 0}
      <p class="muted">{t('speedtest.history.empty')}</p>
    {:else}
      <SpeedHistoryTable rows={speed.history} />
    {/if}
  </section>

</section>

<style>
  .page {
    display: flex;
    flex-direction: column;
    gap: var(--gap);
  }
  .head {
    display: flex;
    align-items: flex-start;
    gap: 14px;
  }
  .titles {
    flex: 1;
    min-inline-size: 0;
  }
  h1 {
    margin: 0;
    font-size: var(--fs-title);
    line-height: 32px;
  }
  .diag {
    margin-block: 2px 0;
    font-size: var(--fs-lead);
    max-inline-size: var(--measure);
  }
  .diag b {
    font-weight: 500;
  }
  .muted,
  .k {
    color: var(--muted);
  }
  .k {
    font-size: var(--fs-small);
  }
  .note {
    margin: 0;
    padding-block: 8px;
    padding-inline: 10px;
    border-radius: var(--radius-control);
    background: var(--raised);
    font-size: var(--fs-small);
  }
  .err {
    margin: 0;
    padding-block: 10px;
    padding-inline: 12px;
    border-radius: var(--radius-control);
    background: var(--pill-negative-bg);
    color: var(--pill-negative-ink);
  }
  .err p {
    margin: 0;
  }
  .err .cause {
    margin-block-start: 4px;
    font-size: var(--fs-small);
    opacity: 0.85;
    text-align: start;
    overflow-wrap: anywhere;
  }
  button.run,
  button.secondary {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    min-block-size: var(--control-height);
    padding-inline: 14px;
  }
  button.secondary {
    background: var(--secondary);
    border-color: var(--secondary);
    font-weight: 500;
  }
  button.link {
    all: unset;
    display: inline-flex;
    align-items: center;
    gap: 4px;
    color: var(--text);
    text-decoration: underline;
    text-underline-offset: 2px;
    cursor: pointer;
  }
  button.link:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  .card {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius-card);
    padding-block: var(--card-pad-block);
    padding-inline: var(--card-pad-inline);
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .cardhead {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    flex-wrap: wrap;
    gap: 4px 12px;
  }
  .cardhead.col {
    flex-direction: column;
    align-items: flex-start;
  }
  .strong {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    font-weight: 600;
    font-size: var(--fs-section);
    margin: 0;
    font-family: inherit;
  }
  .strong :global(svg) {
    color: var(--accent);
  }
  /* The order follows the test, so the numbers fill in left to right. Download is what most
     people came to see, so it gets the room and the size. */
  .stages {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(0, 1.35fr) minmax(0, 1.1fr) minmax(0, 1.35fr);
    gap: 6px 18px;
  }
  @media (max-width: 760px) {
    .stages {
      grid-template-columns: repeat(2, minmax(0, 1fr));
    }
  }
  .status {
    min-block-size: 1.5em;
  }
  .bar.resting {
    visibility: hidden;
  }
  .stage.lead .figure {
    font-size: var(--fs-figure);
    font-weight: 600;
  }
  /* Spectrum: the download is the answer, so it is read first. */
  :global(:root[data-design='spectrum']) .stage.lead {
    order: -1;
  }
  .topcard {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 14px;
    flex-wrap: wrap;
  }
  /* Label, figure and bar share the page's rows, so figures of two sizes sit on one baseline. */
  .stage {
    display: grid;
    grid-row: span 3;
    grid-template-rows: subgrid;
  }
  .stage .figure {
    align-self: last baseline;
  }
  .label :global(svg) {
    flex: none;
  }
  .label {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 4px;
  }
  .figure {
    font-size: var(--fs-title);
    line-height: 36px;
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 4px;
  }
  .bar {
    block-size: 4px;
    border-radius: 2px;
    background: var(--q-track);
    overflow: hidden;
  }
  .bar i {
    display: block;
    block-size: 4px;
    background: var(--accent);
    transition: inline-size 0.2s linear;
  }
  .bar.opaque i {
    opacity: 0.35;
  }
  .lat-high {
    color: var(--q-poor);
  }
  .lat-moderate {
    color: var(--q-fair);
  }
  .settings {
    display: flex;
    flex-wrap: wrap;
    gap: 16px 28px;
  }
  fieldset {
    border: 0;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  legend {
    font-weight: 500;
    padding: 0;
    margin-block-end: 6px;
  }
  .seg {
    display: inline-flex;
    flex-wrap: wrap;
    padding: 2px;
    gap: 2px;
    border-radius: var(--radius-control);
    background: var(--raised);
  }
  .seg label {
    position: relative;
    display: inline-flex;
    align-items: center;
    padding-block: 5px;
    padding-inline: 12px;
    border-radius: var(--radius-chip);
    cursor: pointer;
    color: var(--muted);
    transition:
      background-color var(--t-fast) var(--ease),
      color var(--t-fast) var(--ease);
  }
  .seg label:hover {
    color: var(--text);
  }
  .seg label.on {
    background: var(--surface);
    color: var(--text);
    font-weight: 500;
    box-shadow: var(--shadow-thumb), 0 0 0 1px var(--control);
  }
  .seg label:has(input:focus-visible) {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
  .seg input {
    position: absolute;
    opacity: 0;
    inline-size: 1px;
    block-size: 1px;
    margin: 0;
  }
  fieldset:disabled .seg label {
    cursor: not-allowed;
    opacity: 0.55;
  }
  .terms {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding-block: 12px;
    padding-inline: 14px;
    border-radius: var(--radius-control);
    background: var(--raised);
  }
  .terms h3 {
    margin: 0;
    font-size: var(--fs-section);
  }
  .terms ul {
    margin: 0;
    padding-inline-start: 18px;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .row {
    display: flex;
    flex-wrap: wrap;
    gap: 8px 16px;
    align-items: center;
  }

  /* Spectrum fills the width: the result beside the test's settings, the table under both. */
  @media (min-width: 1100px) {
    :global(:root[data-design='spectrum']) .page {
      display: grid;
      grid-template-columns: minmax(0, 2fr) minmax(0, 1fr);
      align-items: stretch;
    }
    :global(:root[data-design='spectrum']) .page > * {
      grid-column: 1 / -1;
    }
    :global(:root[data-design='spectrum']) .page > section.card:nth-of-type(1) {
      grid-column: 2;
      grid-row: 2;
    }
    :global(:root[data-design='spectrum']) .page > section.card:nth-of-type(2) {
      grid-column: 1;
      grid-row: 2;
    }
    :global(:root[data-design='spectrum']) .page > section.card:nth-of-type(1) .settings {
      flex-direction: column;
      align-items: stretch;
    }
  }

  .lanes {
    display: flex;
    flex-direction: column;
    gap: 10px;
    margin-block-start: 8px;
  }
  .lane {
    display: grid;
    grid-template-columns: 110px minmax(0, 1fr);
    align-items: center;
    gap: 12px;
  }
  .lane i {
    display: block;
    block-size: 8px;
    border-radius: 4px;
    background: var(--faint);
  }
  .lane i.main {
    block-size: 24px;
    border-radius: 6px;
    background: var(--accent);
  }
  /* Spectrum: the verdict is the page's headline, and the page name steps back to a label. */
  :global(:root[data-design='spectrum']) h1 {
    font-size: var(--fs-small);
    font-weight: 500;
    line-height: 18px;
    color: var(--muted);
  }
  :global(:root[data-design='spectrum']) .diag b {
    display: block;
    font-family: var(--font-heading);
    font-size: var(--fs-verdict);
    font-weight: 600;
    line-height: 1.3;
  }
</style>
