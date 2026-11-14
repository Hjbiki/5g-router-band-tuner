<script lang="ts">
  import ArrowLeftRight from '@lucide/svelte/icons/arrow-left-right'
  import ArrowRight from '@lucide/svelte/icons/arrow-right'
  import Bookmark from '@lucide/svelte/icons/bookmark'
  import Ban from '@lucide/svelte/icons/ban'
  import Check from '@lucide/svelte/icons/check'
  import CheckCheck from '@lucide/svelte/icons/check-check'
  import ChevronDown from '@lucide/svelte/icons/chevron-down'
  import CircleCheck from '@lucide/svelte/icons/circle-check'
  import ListChecks from '@lucide/svelte/icons/list-checks'
  import RadioTower from '@lucide/svelte/icons/radio-tower'
  import Snail from '@lucide/svelte/icons/snail'
  import Star from '@lucide/svelte/icons/star'
  import TriangleAlert from '@lucide/svelte/icons/triangle-alert'
  import X from '@lucide/svelte/icons/x'
  import { onMount } from 'svelte'
  import { api } from '../lib/backend'
  import { TONE_LABEL_KEY, bandTones, lastBandRecord } from '../lib/bandHistory'
  import { ensureSpeedTest, speed } from '../lib/speedtest.svelte'
  import { ensureScan, scan, scanActive } from '../lib/scan.svelte'
  import BandLastResult from './BandLastResult.svelte'
  import type { BandOutcome } from '../lib/bindings/BandOutcome'
  import { app, bandEdit, pushToast, updateSettings } from '../lib/store.svelte'
  import { t, type MessageKey } from '../lib/i18n'
  import { errorText } from '../lib/errors'
  import { formatNumber, isolate } from '../lib/format'
  import {
    LTE_BAND_INFO,
    bandChange,
    bandsOf,
    changeClosed,
    groupBands,
    hasBand,
    isEmptyMask,
    restrictToSupported,
    sameConfig,
    splitDuration,
    supportedBands,
    templateParts,
    toggleBand,
    withBand,
  } from '../lib/bands'
  import type { ApplyPreview } from '../lib/bindings/ApplyPreview'
  import type { NetConfig } from '../lib/bindings/NetConfig'
  import type { NetworkMode } from '../lib/bindings/NetworkMode'
  import type { NrOption } from '../lib/bindings/NrOption'
  import type { Preset } from '../lib/bindings/Preset'
  import type { Settings } from '../lib/bindings/Settings'
  import InfoTip from './ui/InfoTip.svelte'
  import DuplexHeading from './ui/DuplexHeading.svelte'
  import BandToneMark from './ui/BandToneMark.svelte'
  import SendPreview from './SendPreview.svelte'
  import LoginNeeded from './ui/LoginNeeded.svelte'
  import BandMap from './spectrum/BandMap.svelte'
  import { needsLogin } from '../lib/login'

  const PREVIEW_DEBOUNCE_MS = 250
  /** Used only until settings have loaded; the same as the backend defaults. */
  const DEFAULT_TIMERS = { graceS: 90, lossS: 60, probationS: 180 }

  const snap = $derived(app.snapshot)
  const caps = $derived(snap?.capabilities ?? null)
  const current = $derived(snap?.config ?? null)
  const phase = $derived(snap?.safeApply.phase)
  // Between the two steps of a primary-band apply the phase reads "committed", but the change
  // isn't finished until the other bands are back.
  const sequence = $derived(snap?.safeApply.sequence ?? null)
  const closed = $derived(changeClosed(phase) && !sequence)
  const digits = $derived(app.settings?.digits ?? 'western')
  const serving = $derived(snap?.signal?.lte?.band ?? null)
  const primary = $derived(app.settings?.automation.primaryBand ?? null)

  // The draft lives in the store, so switching tabs keeps unsaved edits.
  // Follow the router until the user starts editing.
  $effect(() => {
    if (!bandEdit.dirty && current) bandEdit.draft = $state.snapshot(current) as NetConfig
  })

  // When a change finishes (kept or reverted) show what the router has now.
  $effect(() => {
    if (closed && !bandEdit.wasClosed) bandEdit.dirty = false
    bandEdit.wasClosed = closed
  })

  // Past results colour the chips. A history that can't be read just leaves them neutral.
  let outcomes: BandOutcome[] = $state([])
  let scanPeriods: [number, number][] = $state([])
  const loadOutcomes = () =>
    Promise.all([api.bandOutcomes(), api.scanPeriods()])
      .then(([o, p]) => {
        outcomes = o
        scanPeriods = p
      })
      .catch(() => {})
  onMount(() => {
    void loadOutcomes()
    void ensureSpeedTest()
    void ensureScan()
  })
  // A finished change may be new evidence.
  $effect(() => {
    if (closed) void loadOutcomes()
  })
  const tones = $derived(bandTones(speed.history, outcomes, scan.file?.results ?? []))
  const scanRunning = $derived(scanActive())
  // Each past result has its own shape (BandToneMark), so it never rests on the colour alone.

  const supported = $derived(caps ? supportedBands(caps.supportedLte) : [])
  /** The Spectrum design draws the bands as a map of the spectrum instead of grouped chips. */
  const spectrum = $derived(app.settings?.design === 'spectrum')
  const records = $derived(
    new Map(supported.map((b) => [b, lastBandRecord(b, speed.history, outcomes, scan.file?.results ?? [], scanPeriods)] as const)),
  )
  /** The legend explains only the marks that are on a chip right now. */
  const seenTones = $derived(new Set(supported.map((b) => tones.get(b)).filter((x) => x !== undefined)))
  const groups = $derived(groupBands(supported))
  const currentBands = $derived(bandsOf(current))
  const draftBands = $derived(bandsOf(bandEdit.draft))
  const allowsAll = (list: number[]) => supported.length > 0 && supported.every((b) => list.includes(b))

  const modeKey = (m: NetworkMode) => (m.kind === 'raw' ? `raw:${m.value}` : m.kind)
  const modeList = $derived.by(() => {
    const list = caps ? [...caps.modes] : []
    if (bandEdit.draft && !list.some((m) => modeKey(m) === modeKey(bandEdit.draft!.networkMode))) list.push(bandEdit.draft.networkMode)
    return list
  })
  const modeLabel = (m: NetworkMode) =>
    m.kind === 'raw' ? t('bands.modes.raw', { value: m.value }) : t(`bands.modes.${m.kind}`)
  const nrLabel = (o: NrOption | null) => (o ? t(`bands.nrOption.${o}`) : t('app.unknown'))

  const nrOptions = $derived(caps?.nrOptions ?? [])
  const showNr = $derived(nrOptions.length > 0 && bandEdit.draft !== null && bandEdit.draft.nrOption !== null)
  const nsaWarning = $derived(bandEdit.draft?.networkMode.kind === 'nrOnly' && bandEdit.draft.nrOption === 'nsa')

  function edit(next: NetConfig) {
    bandEdit.draft = next
    // Undoing every edit returns to following the router (and hides "Discard edits").
    bandEdit.dirty = current === null || !sameConfig(next, current)
  }
  const onToggle = (b: number) => bandEdit.draft && edit(toggleBand(bandEdit.draft, b))
  const onMode = (key: string) => {
    const m = modeList.find((x) => modeKey(x) === key)
    if (bandEdit.draft && m) edit({ ...bandEdit.draft, networkMode: m })
  }
  const onNr = (o: NrOption) => bandEdit.draft && edit({ ...bandEdit.draft, nrOption: o })

  function applyPresetConfig(c: NetConfig) {
    if (!bandEdit.draft || !caps) return
    const bands = restrictToSupported(c.lteBands, caps.supportedLte)
    const mode = modeList.find((m) => modeKey(m) === modeKey(c.networkMode)) ?? bandEdit.draft.networkMode
    const nr = bandEdit.draft.nrOption !== null && c.nrOption && nrOptions.includes(c.nrOption) ? c.nrOption : bandEdit.draft.nrOption
    edit({ ...bandEdit.draft, lteBands: bands, networkMode: mode, nrOption: nr })
  }
  const presetAuto = () =>
    caps &&
    applyPresetConfig({
      networkMode: { kind: 'auto' },
      lteBands: caps.supportedLte,
      legacyBandRaw: bandEdit.draft?.legacyBandRaw ?? '0',
      // Auto means all bands; the 5G option stays what it is now.
      nrOption: bandEdit.draft?.nrOption ?? null,
    })

  /**
   * Saves a settings change (queued behind any other settings save, applied to the newest
   * settings), then updates the store; errors become a toast.
   */
  async function saveSettings(change: (s: Settings) => Settings): Promise<void> {
    try {
      await updateSettings(change)
    } catch (e) {
      pushToast(errorText(e), 'error')
    }
  }

  let presetName = $state('')
  async function savePreset() {
    const name = presetName.trim()
    if (!name || !bandEdit.draft || !app.settings || isEmptyMask(bandEdit.draft)) return
    const preset: Preset = { name, config: $state.snapshot(bandEdit.draft) as NetConfig }
    await saveSettings((s) => ({ ...s, presets: [...s.presets.filter((p) => p.name !== name), preset] }))
    presetName = ''
  }
  const deletePreset = (name: string) =>
    saveSettings((s) => ({ ...s, presets: s.presets.filter((p) => p.name !== name) }))

  // One starred band. Starring a band that isn't selected also selects it; starring the
  // primary again clears it.
  let savingPrimary = $state(false)
  async function onStar(b: number) {
    if (!app.settings || savingPrimary) return
    const next = primary === b ? null : b
    if (next !== null && bandEdit.draft) {
      const selected = withBand(bandEdit.draft, b)
      if (selected !== bandEdit.draft) edit(selected)
    }
    savingPrimary = true
    try {
      // Without a primary band there is nothing to re-lock to; clearing the star also turns
      // re-lock off, so starring a band later doesn't quietly re-arm it.
      await saveSettings((s) => ({
        ...s,
        automation: { ...s.automation, primaryBand: next, ...(next === null ? { relockPrimary: false } : {}) },
      }))
      bandEdit.primaryEdited = next !== null
    } finally {
      savingPrimary = false
    }
  }

  // The preview is the backend's own rendering of what Apply will send.
  let preview: ApplyPreview | null = $state(null)
  let previewState: 'loading' | 'ready' | 'failed' = $state('loading')
  let previewKey: string | null = $state(null)
  let seq = 0
  const keyOf = (c: NetConfig) => JSON.stringify(c)

  // Depend on content keys, not object identity, so identical snapshots do not refetch.
  const draftKey = $derived(bandEdit.draft ? keyOf($state.snapshot(bandEdit.draft) as NetConfig) : null)
  const capsReady = $derived(caps !== null)
  $effect(() => {
    const key = draftKey
    if (key === null || !capsReady) return
    const cfg = JSON.parse(key) as NetConfig
    if (isEmptyMask(cfg)) return
    const mine = ++seq
    previewState = 'loading'
    const timer = setTimeout(async () => {
      try {
        const p = await api.previewConfig(cfg)
        if (mine !== seq) return
        preview = p
        previewKey = key
        previewState = 'ready'
      } catch {
        if (mine === seq) previewState = 'failed'
      }
    }, PREVIEW_DEBOUNCE_MS)
    return () => clearTimeout(timer)
  })

  const changed = $derived(bandEdit.draft !== null && current !== null && !sameConfig(bandEdit.draft, current))
  const empty = $derived(bandEdit.draft !== null && isEmptyMask(bandEdit.draft))
  const previewFresh = $derived(bandEdit.draft !== null && previewKey === keyOf($state.snapshot(bandEdit.draft) as NetConfig))

  const blockReason = $derived.by((): MessageKey | null => {
    if (!snap || !caps || !bandEdit.draft || snap.session.kind !== 'loggedIn') return 'bands.reason.notReady'
    if (scanRunning) return 'bands.reason.scanRunning'
    if (snap.foreignJournal) return 'bands.reason.foreignJournal'
    if (!caps.bandEditAllowed) return 'bands.reason.editNotAllowed'
    if (!closed) return 'bands.reason.changeInProgress'
    if (empty) return 'bands.reason.noBands'
    if (!changed && !primaryPending) return 'bands.reason.unchanged'
    if (previewState === 'failed') return 'bands.reason.previewFailed'
    if (!previewFresh || previewState !== 'ready') return 'bands.reason.previewPending'
    return null
  })
  const panelLocked = $derived(!caps?.bandEditAllowed || !closed || scanRunning)

  let applying = $state(false)
  async function apply() {
    if (!bandEdit.draft || blockReason || applying) return
    applying = true
    try {
      await api.applyConfig($state.snapshot(bandEdit.draft) as NetConfig)
      bandEdit.dirty = true
      bandEdit.primaryEdited = false
    } catch (e) {
      pushToast(errorText(e), 'error')
    } finally {
      applying = false
    }
  }
  function discard() {
    bandEdit.dirty = false
    bandEdit.primaryEdited = false
  }

  // "What happens when you apply" uses the user's own timers.
  const timers = $derived(app.settings?.timers ?? DEFAULT_TIMERS)
  const revertOnLoss = $derived(app.settings?.automation.revertOnLoss ?? true)
  function duration(seconds: number): string {
    const { m, s } = splitDuration(seconds)
    const n = (x: number) => isolate(formatNumber(x, { digits }))
    if (m === 0) return t('bands.duration.s', { s: n(s) })
    if (s === 0) return t('bands.duration.min', { m: n(m) })
    return t('bands.duration.minS', { m: n(m), s: n(s) })
  }
  // With a starred band in a draft that also has other bands, Apply runs in two steps (the
  // primary band alone first, then the rest), so the router reconnects twice.
  const twoStep = $derived(
    primary !== null && bandEdit.draft !== null && hasBand(bandEdit.draft, primary) && draftBands.length > 1,
  )
  // Same bands, but the star moved or the router isn't on the starred band: Apply settles it there.
  const primaryPending = $derived(
    twoStep && !changed && (bandEdit.primaryEdited || (serving !== null && serving !== primary)),
  )
  const steps = $derived.by((): { text: string; off: boolean }[] => {
    const list = [
      { text: t('bands.change.stepSaved'), off: false },
      { text: t('bands.change.stepReconnect', { grace: duration(timers.graceS) }), off: false },
      revertOnLoss
        ? {
            text: t('bands.change.stepTrial', { probation: duration(timers.probationS), loss: duration(timers.lossS) }),
            off: false,
          }
        : { text: t('bands.change.stepTrialNoRevert', { probation: duration(timers.probationS) }), off: true },
    ]
    if (twoStep && primary !== null) {
      list.splice(1, 0, {
        text: t('bands.change.stepTwoStep', { band: isolate(`B${primary}`), grace: duration(timers.graceS) }),
        off: false,
      })
    }
    return list
  })
  // Collapsed to one sentence, except when the steps differ from what the sentence says.
  const stepsStayOpen = $derived(!revertOnLoss || twoStep)

  const countIn = (list: number[]) => list.filter((b) => bandEdit.draft && hasBand(bandEdit.draft, b)).length
  const modeChanged = $derived(
    bandEdit.draft !== null && current !== null && modeKey(bandEdit.draft.networkMode) !== modeKey(current.networkMode),
  )
  const bandsChanged = $derived(draftBands.join() !== currentBands.join())
  const addedBands = $derived(draftBands.filter((b) => !currentBands.includes(b)))
  const removedBands = $derived(currentBands.filter((b) => !draftBands.includes(b)))
  const nrChanged = $derived(bandEdit.draft !== null && current !== null && bandEdit.draft.nrOption !== current.nrOption)
</script>

{#snippet bandNames(list: number[])}
  {#if list.length === 0}{t('bands.change.none')}{:else}{#each list as b, i (b)}{#if i > 0}{t(
          'bands.listSep',
        )}{/if}<bdi dir="ltr">B{b}</bdi>{/each}{/if}
{/snippet}

<section class="bands" aria-labelledby="bands-title">
  <header class="head">
    <h1 id="bands-title">{t('bands.title')}</h1>
    {#if current && caps}
      <p class="lede">
        {#each templateParts(t(serving !== null ? 'bands.summaryServing' : 'bands.summary')) as part, i (i)}{#if 'text' in part}{part.text}{:else if part.slot === 'allowed'}<strong
              >{#if allowsAll(currentBands)}{t('bands.allSupported')}{:else}{@render bandNames(currentBands)}{/if}</strong
            >{:else if part.slot === 'serving'}<strong><bdi dir="ltr">B{serving}</bdi></strong>{/if}{/each}
        {t('bands.pickHint')}
      </p>
    {/if}
  </header>

  {#if scanRunning}
    <p class="scan-note" role="status">
      {t('bands.reason.scanRunning')}
      <button type="button" class="link" onclick={() => (app.tab = 'scan')}>{t('bands.seeScan')}</button>
    </p>
  {/if}

  {#if (!bandEdit.draft || !caps) && needsLogin(app.snapshot?.session)}
    <LoginNeeded what="login.needed.bands" />
  {:else if !bandEdit.draft || !caps}
    <p class="hint" role="status">{t('bands.noData')}</p>
  {:else}
    <div class="layout">
      <section class="card choice" aria-label={t('bands.choiceLabel')}>
        <div class="plain">
          <div class="presets" data-testid="quick-picks">
            <h3 class="lbl"><Bookmark size={16} aria-hidden="true" />{t('bands.presets')}</h3>
            <InfoTip label={t('metrics.whatIs', { name: t('bands.presets') })} text={t('bands.help.presets')} />
            <span class="all-bands">
              <button type="button" class="chip" disabled={panelLocked} aria-describedby="preset-all-hint" onclick={presetAuto}>
                <CheckCheck size={15} aria-hidden="true" />{t('bands.presetAuto')}
              </button>
              <span id="preset-all-hint" class="hint">{t('bands.presetAutoHint')}</span>
            </span>
            {#each app.settings?.presets ?? [] as p (p.name)}
              <span class="chip-group">
                <button type="button" class="chip" disabled={panelLocked} onclick={() => applyPresetConfig(p.config)}><bdi>{p.name}</bdi></button
                ><button
                  type="button"
                  class="chip del"
                  disabled={panelLocked}
                  aria-label={`${t('bands.deletePreset')}: ${p.name}`}
                  onclick={() => deletePreset(p.name)}><X size={14} aria-hidden="true" /></button
                >
              </span>
            {/each}
          </div>
          <div class="controls">
            <div class="field">
              <div class="lbl-row">
                <label class="lbl" for="mode-select">{t('bands.mode')}</label>
                <InfoTip label={t('metrics.whatIs', { name: t('bands.mode') })} text={t('bands.help.mode')} />
              </div>
              <select
                id="mode-select"
                disabled={panelLocked}
                value={modeKey(bandEdit.draft.networkMode)}
                onchange={(e) => onMode(e.currentTarget.value)}
              >
                {#each modeList as m (modeKey(m))}
                  <option value={modeKey(m)}>{modeLabel(m)}</option>
                {/each}
              </select>
            </div>

            {#if showNr}
              <div class="field">
                <div class="lbl-row">
                  <span class="lbl" id="nr-label">{t('bands.nrOptionLabel')}</span>
                  <InfoTip label={t('metrics.whatIs', { name: 'NSA / SA' })} text={t('metrics.help.nsaSa')} />
                </div>
                <div class="seg" role="radiogroup" aria-label={t('bands.nrOptionLabel')}>
                  {#each nrOptions as o (o)}
                    <label class:on={bandEdit.draft.nrOption === o}>
                      <input
                        type="radio"
                        name="nr-option"
                        disabled={panelLocked}
                        value={o}
                        checked={bandEdit.draft.nrOption === o}
                        onchange={() => onNr(o)}
                      />
                      {t(`bands.nrOption.${o}`)}
                    </label>
                  {/each}
                </div>
              </div>
            {/if}
          </div>
          {#if nsaWarning}<p class="warn" role="alert">{t('bands.nsaNeedsAnchor')}</p>{/if}

          {#if spectrum}
            <BandMap {supported} draft={bandEdit.draft} {current} {serving} {records} disabled={panelLocked} {onToggle} />
            <div class="field primary-pick">
              <div class="lbl-row">
                <label class="lbl" for="primary-select">{t('spectrum.map.primary')}</label>
                <InfoTip label={t('bands.primaryHelpLabel')} text={t('bands.primaryHelp')} />
              </div>
              <select
                id="primary-select"
                disabled={panelLocked || !app.settings || savingPrimary}
                value={primary === null ? '' : String(primary)}
                onchange={(e) => {
                  const v = e.currentTarget.value
                  if (v === '') {
                    if (primary !== null) void onStar(primary)
                  } else void onStar(Number(v))
                }}
              >
                <option value="">{t('spectrum.map.primaryNone')}</option>
                {#each supported as b (b)}<option value={String(b)}>B{b}</option>{/each}
              </select>
            </div>
          {:else}
          {#each [['fdd', groups.fdd], ['tdd', groups.tdd], ['sdl', groups.sdl]] as const as [duplex, list] (duplex)}
            {#if list.length}
              <div class="group" data-testid="band-group-{duplex}">
                <DuplexHeading {duplex} count={countIn(list)} total={list.length} />
                <div class="keys">
                  {#each list as b (b)}
                    {@const info = LTE_BAND_INFO[b]}
                    {@const on = hasBand(bandEdit.draft, b)}
                    {@const delta = bandChange(bandEdit.draft, current, b)}
                    {@const tone = tones.get(b)}
                    <div class="key-wrap">
                      <button
                        type="button"
                        class="key duplex-{info?.duplex ?? 'fdd'} {tone ? `tone-${tone}` : ''}"
                        class:on
                        aria-pressed={on}
                        disabled={panelLocked}
                        title={[serving === b ? t('bands.servingNow') : null, tone ? t(TONE_LABEL_KEY[tone]) : null]
                          .filter(Boolean)
                          .join(' · ') || undefined}
                        onclick={() => onToggle(b)}
                      >
                        <span class="name"
                          ><bdi dir="ltr">B{b}</bdi>{#if on}<Check class="tick" size={14} aria-hidden="true" />{/if}{#if serving === b}{' '}<span
                              class="serving"
                              ><RadioTower size={14} aria-hidden="true" /><span class="sr-only">{t('bands.servingNow')}</span></span
                            >{/if}{#if tone}{' '}<BandToneMark {tone} />{/if}</span
                        >
                        {#if info}
                          <span class="mhz num"
                            ><bdi>{t('bands.mhz', { mhz: isolate(formatNumber(info.mhz, { digits })) })}</bdi
                            >{#if info.duplex === 'sdl'}{' · '}{t('bands.downOnly')}{/if}</span
                          >
                        {/if}
                        {#if delta}
                          <span class="marks"><span class="mark {delta}">{t(`bands.${delta}`)}</span></span>
                        {/if}
                      </button>
                      <button
                        type="button"
                        class="star"
                        class:starred={primary === b}
                        aria-pressed={primary === b}
                        aria-label={t('bands.makePrimary', { band: `B${b}` })}
                        title={t('bands.makePrimary', { band: `B${b}` })}
                        disabled={panelLocked || !app.settings || savingPrimary}
                        onclick={() => onStar(b)}
                      >
                        <Star size={14} fill={primary === b ? 'currentColor' : 'none'} aria-hidden="true" />
                      </button>
                      <span class="key-info">
                        <InfoTip label={t('bands.last.label', { band: `B${b}` })}>
                          <BandLastResult band={b} record={lastBandRecord(b, speed.history, outcomes, scan.file?.results ?? [], scanPeriods)} />
                        </InfoTip>
                      </span>
                    </div>
                  {/each}
                </div>
              </div>
            {/if}
          {/each}
          {#if serving !== null || seenTones.size > 0}
            <ul class="legend" aria-label={t('bands.legend.label')}>
              {#if serving !== null}<li class="serving"><RadioTower size={14} aria-hidden="true" />{t('bands.legend.serving')}</li>{/if}
              {#if seenTones.has('good')}<li class="tone-good"><CircleCheck size={14} aria-hidden="true" />{t('bands.legend.good')}</li>{/if}
              {#if seenTones.has('slow')}<li class="tone-slow"><Snail size={14} aria-hidden="true" />{t('bands.legend.slow')}</li>{/if}
              {#if seenTones.has('noService')}<li class="tone-noService"><Ban size={14} aria-hidden="true" />{t('bands.legend.noService')}</li>{/if}
            </ul>
          {/if}
          <p class="primary-help">
            <Star size={12} fill={primary !== null ? 'currentColor' : 'none'} aria-hidden="true" />
            {#if primary !== null}
              <span>{t('bands.primaryIs', { band: isolate(`B${primary}`) })}</span>
            {:else}
              <span>{t('bands.primaryNone')}</span>
            {/if}
            <InfoTip label={t('bands.primaryHelpLabel')} text={t('bands.primaryHelp')} />
          </p>
          {/if}

          <div class="save-row">
            <span class="hint">{t('bands.savePresetHint')}</span>
            <span class="save-preset">
              <input
                type="text"
                maxlength="40"
                disabled={panelLocked}
                aria-label={t('bands.presetName')}
                placeholder={t('bands.presetName')}
                bind:value={presetName}
              />
              <button type="button" class="chip" disabled={panelLocked || !presetName.trim() || empty} onclick={savePreset}
                >{t('bands.savePreset')}</button
              >
            </span>
          </div>
        </div>
      </section>

      <!-- A narrow window stacks the panel far below the chips, so this says there's a change to review. -->
      {#if (changed || primaryPending) && closed}
        <div class="review-bar">
          <span class="review-route">
            {@render bandNames(currentBands)}<ArrowRight class="review-arrow" size={14} aria-hidden="true" /><span class="sr-only"
              >{t('safeApply.routeTo')}</span
            >{@render bandNames(draftBands)}
          </span>
          <button
            type="button"
            class="primary"
            onclick={() => document.getElementById('change-title')?.scrollIntoView({ behavior: 'smooth', block: 'start' })}
          >
            {t('bands.change.title')}<ChevronDown size={16} aria-hidden="true" />
          </button>
        </div>
      {/if}

      <aside class="card change" class:floating={closed} aria-labelledby="change-title">
        <h2 id="change-title"><ArrowLeftRight size={18} aria-hidden="true" />{t('bands.change.title')}</h2>
        <dl>
          <dt>{t('bands.change.now')}</dt>
          <dd>{#if allowsAll(currentBands)}{t('bands.allSupported')}{:else}{@render bandNames(currentBands)}{/if}</dd>
          {#if bandsChanged}
            <dt>{t('bands.change.after')}</dt>
            <dd class="after">
              {#if allowsAll(draftBands)}{t('bands.allSupported')}{:else}{@render bandNames(draftBands)}{/if}
              <span class="delta" data-testid="band-delta">
                {#each addedBands as b (b)}<span class="mark added"><bdi dir="ltr">+B{b}</bdi></span>{/each}
                {#each removedBands as b (b)}<span class="mark removed"><bdi dir="ltr">−B{b}</bdi></span>{/each}
              </span>
            </dd>
          {/if}
          {#if modeChanged && current}
            <dt>{t('bands.change.mode')}</dt>
            <dd>
              {modeLabel(bandEdit.draft.networkMode)}
              <span class="note">{t('bands.change.was', { value: modeLabel(current.networkMode) })}</span>
            </dd>
          {/if}
          {#if showNr && nrChanged && current}
            <dt>{t('bands.change.nr')}</dt>
            <dd>
              {nrLabel(bandEdit.draft.nrOption)}
              <span class="note">{t('bands.change.was', { value: nrLabel(current.nrOption) })}</span>
            </dd>
          {/if}
        </dl>

        {#if changed || primaryPending}
          <p class="everyone" role="note"><TriangleAlert size={16} aria-hidden="true" /><span>{t('bands.change.everyone')}</span></p>

          <details class="steps" open={stepsStayOpen} data-testid="apply-steps">
            <summary>
              <ListChecks size={16} aria-hidden="true" /><span class="steps-title">{t('bands.change.whatHappens')}</span>
              <ChevronDown class="chevron" size={16} aria-hidden="true" />
            </summary>
            {#if !stepsStayOpen}
              <p class="steps-short">{t('bands.change.short', { grace: duration(timers.graceS) })}</p>
            {/if}
            <ol>
              {#each steps as s, i (i)}<li class:off={s.off}>{s.text}</li>{/each}
            </ol>
          </details>

          <SendPreview {preview} state={previewState} />
        {:else if closed}
          <p class="idle" data-testid="change-idle">{t('bands.change.idle')}</p>
        {/if}

        <div class="actions">
          <button
            type="button"
            class="primary"
            disabled={blockReason !== null || applying}
            aria-describedby={blockReason ? 'apply-reason' : undefined}
            onclick={apply}>{t('bands.applySafely')}</button
          >
          {#if bandEdit.dirty && !panelLocked}
            <button type="button" class="secondary" onclick={discard}>{t('bands.resetEdits')}</button>
          {/if}
          {#if blockReason}<p id="apply-reason" class="hint">{t(blockReason)}</p>{/if}
        </div>
      </aside>
    </div>
  {/if}
</section>

<style>
  .bands {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }
  .scan-note {
    margin: 0;
    color: var(--muted);
  }
  .link {
    all: unset;
    color: var(--accent);
    font-weight: 500;
    text-decoration: underline;
    cursor: pointer;
  }
  .link:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  .head h1 {
    margin: 0;
    font-size: var(--fs-title);
    line-height: 32px;
  }
  .lede {
    margin-block: 2px 0;
    color: var(--muted);
    max-inline-size: var(--measure);
  }
  .lede strong {
    color: var(--text);
    font-weight: 500;
  }
  .layout {
    display: grid;
    grid-template-columns: minmax(0, 1fr) clamp(264px, 28%, 380px);
    align-items: start;
    gap: var(--gap);
  }
  .choice {
    align-self: stretch;
  }
  /* Beside the chips, Apply stays in view however far the page scrolls. */
  .change.floating {
    position: sticky;
    inset-block-start: 8px;
    max-block-size: calc(100vh - var(--titlebar-height) - 24px);
    overflow-y: auto;
  }
  .review-bar {
    display: none;
  }
  @media (max-width: 820px) {
    .layout {
      grid-template-columns: minmax(0, 1fr);
    }
    .change.floating {
      position: static;
      max-block-size: none;
    }
    .review-bar {
      position: sticky;
      inset-block-end: 8px;
      z-index: 2;
      display: flex;
      align-items: center;
      justify-content: space-between;
      gap: 12px;
      padding-block: 8px;
      padding-inline: 14px 8px;
      border: 1px solid var(--border);
      border-radius: var(--radius-card);
      background: var(--surface);
      box-shadow: var(--shadow-pop);
      animation: pop-in var(--t-enter) var(--ease);
    }
    .review-route {
      min-inline-size: 0;
      font-weight: 500;
    }
    .review-route :global(.review-arrow) {
      margin-inline: 6px;
      vertical-align: -2px;
    }
    /* The arrow points the reading direction. */
    :global([dir='rtl']) .review-route :global(.review-arrow) {
      transform: scaleX(-1);
    }
    .review-bar button {
      flex: none;
      display: inline-flex;
      align-items: center;
      gap: 6px;
      block-size: var(--control-height);
      padding-block: 0;
      font-weight: 500;
    }
  }
  .card {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius-card);
    padding-block: var(--card-pad-block);
    padding-inline: var(--card-pad-inline);
    min-inline-size: 0;
  }
  .plain {
    min-inline-size: 0;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }
  .controls {
    display: flex;
    flex-wrap: wrap;
    align-items: end;
    gap: 16px 24px;
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .lbl {
    font-weight: 500;
  }
  .lbl-row {
    display: flex;
    align-items: center;
    gap: 2px;
    min-block-size: 22px;
  }
  .field select {
    min-inline-size: 170px;
    block-size: 32px;
    padding-block: 0;
  }
  .seg {
    display: inline-flex;
    flex-wrap: wrap;
    gap: 2px;
    padding: 2px;
    border-radius: var(--radius-control);
    background: var(--active);
  }
  .seg label {
    display: inline-flex;
    align-items: center;
    block-size: 28px;
    padding-inline: 12px;
    border-radius: 5px;
    font-size: var(--fs-small);
    color: var(--muted);
    cursor: pointer;
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
  .seg label:focus-within {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }
  .seg input {
    position: absolute;
    opacity: 0;
    pointer-events: none;
  }
  .warn {
    margin: 0;
    padding-block: 8px;
    padding-inline: 12px;
    border-radius: var(--radius-control);
    background: var(--pill-warning-bg);
    color: var(--pill-warning-ink);
  }
  /* The rule above each section's title divides FDD, TDD and SDL. */
  .group {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding-block-start: 14px;
    border-block-start: 1px solid var(--rule);
  }
  h3 {
    margin: 0;
    font-family: var(--font-ui);
    font-size: var(--fs-body);
    font-weight: 500;
  }
  .keys {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(136px, 1fr));
    grid-auto-rows: 1fr;
    gap: 8px;
  }
  /* Every chip in a row is as tall as the tallest, and fills its grid cell. */
  .key-wrap {
    position: relative;
    display: grid;
  }
  .key {
    position: relative;
    display: flex;
    flex-direction: column;
    justify-content: center;
    align-items: flex-start;
    gap: 1px;
    inline-size: 100%;
    min-block-size: 62px;
    padding-block: 6px;
    padding-inline: 10px 28px;
    border: 1px solid var(--border);
    border-radius: var(--radius-control);
    background: var(--surface);
    color: var(--muted);
    text-align: start;
    transition:
      background-color var(--t-fast) var(--ease),
      border-color var(--t-fast) var(--ease),
      box-shadow var(--t-fast) var(--ease),
      color var(--t-fast) var(--ease),
      transform var(--t-fast) var(--ease);
  }
  /* The band's kind (FDD, TDD, SDL) as a stripe on the start edge, apart from the border so the
     past-result edge and the selection never cover it. The group heading says it in words. */
  .key::before {
    content: '';
    position: absolute;
    inset-block: -1px;
    inset-inline-start: -1px;
    inline-size: 3px;
    border-start-start-radius: var(--radius-control);
    border-end-start-radius: var(--radius-control);
    background: var(--duplex, var(--border));
  }
  /* What past results say about a band: a quiet wash and edge. Selection and "serving now" stay on top. */
  .tone-good {
    --tone: var(--q-excellent);
  }
  .tone-slow {
    --tone: var(--q-fair);
  }
  .tone-noService {
    --tone: var(--q-poor);
  }
  .key.tone-good,
  .key.tone-slow,
  .key.tone-noService {
    background: color-mix(in srgb, var(--tone) 9%, var(--surface));
    border-color: color-mix(in srgb, var(--tone) 40%, var(--border));
  }
  .legend {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 16px;
    margin: 0;
    padding: 0;
    list-style: none;
    color: var(--muted);
    font-size: var(--fs-small);
  }
  .legend li {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .swatch {
    display: block;
    inline-size: 14px;
    block-size: 14px;
    border-radius: 4px;
    border: 1px solid var(--border);
    background: var(--surface);
  }
  .legend :global(svg) {
    flex: none;
  }
  .legend .tone-good :global(svg),
  .legend .tone-slow :global(svg),
  .legend .tone-noService :global(svg) {
    color: var(--tone);
  }
  .legend .serving {
    color: var(--accent);
    font-weight: 500;
  }
  .all-bands {
    display: inline-flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 4px 10px;
  }
  .all-bands .chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .all-bands .hint {
    font-size: var(--fs-small);
  }
  .key:hover:not(:disabled) {
    border-color: var(--field-border);
    color: var(--text);
  }
  .key.on,
  .key.on:hover:not(:disabled) {
    border-color: var(--text);
    box-shadow: inset 0 0 0 0.5px var(--text);
    color: var(--text);
  }
  .key:disabled {
    opacity: 1;
    color: var(--faint);
    background: var(--raised);
  }
  .key.on:disabled {
    border-color: var(--field-border);
    box-shadow: none;
    color: var(--muted);
  }
  .name {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font-size: 15px;
    font-weight: 500;
  }
  .mhz {
    color: var(--faint);
    font-size: var(--fs-small);
  }
  .marks {
    display: flex;
    flex-wrap: wrap;
    gap: 0 6px;
    font-size: var(--fs-small);
  }
  /* In the chip's top line, like the tick, so "in use" never changes the chip's height. */
  .serving {
    display: inline-flex;
    color: var(--accent);
  }
  .mark.added {
    color: var(--text);
    font-weight: 500;
  }
  .mark.removed {
    color: var(--q-poor);
  }
  .star {
    position: absolute;
    inset-block-start: 4px;
    inset-inline-end: 4px;
    display: grid;
    place-items: center;
    inline-size: 24px;
    block-size: 24px;
    padding: 0;
    border: none;
    border-radius: 5px;
    background: transparent;
    color: var(--faint);
  }
  .star:hover:not(:disabled) {
    background: var(--active);
    color: var(--text);
  }
  .star.starred {
    color: var(--text);
  }
  .key-info {
    position: absolute;
    inset-block-end: 4px;
    inset-inline-end: 5px;
    display: inline-grid;
  }
  .primary-help {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    margin: 0;
    color: var(--faint);
    font-size: var(--fs-small);
  }
  .presets {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    padding-block-end: 14px;
    border-block-end: 1px solid var(--rule);
  }
  .presets .lbl {
    margin-inline-end: 4px;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    block-size: 30px;
    padding-block: 0;
    padding-inline: 12px;
    font-size: var(--fs-small);
  }
  .chip-group {
    display: inline-flex;
  }
  .chip-group .chip:first-child {
    border-start-end-radius: 0;
    border-end-end-radius: 0;
  }
  .chip.del {
    padding-inline: 8px;
    border-inline-start: none;
    border-start-start-radius: 0;
    border-end-start-radius: 0;
    color: var(--muted);
  }
  .primary-pick select {
    max-inline-size: 220px;
  }
  .save-row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding-block-start: 14px;
    border-block-start: 1px solid var(--rule);
  }
  .save-preset {
    display: inline-flex;
    gap: 6px;
  }
  .delta {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    margin-block-start: 4px;
  }
  .idle {
    margin: 0;
    color: var(--muted);
    font-size: var(--fs-small);
  }
  .save-preset input {
    block-size: 30px;
    inline-size: 150px;
    font-size: var(--fs-small);
  }
  .change {
    display: flex;
    flex-direction: column;
    gap: 14px;
    padding-inline: 18px;
  }
  .change h2 {
    margin: 0;
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: var(--fs-section);
    font-weight: 600;
  }
  .change h2 :global(svg),
  .presets h3 :global(svg),
  .steps summary > :global(svg:first-child) {
    color: var(--accent);
    flex: none;
  }
  .presets h3 {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  dl {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 6px 12px;
    margin: 0;
  }
  dt {
    color: var(--faint);
    font-size: var(--fs-small);
    line-height: 20px;
  }
  dd {
    margin: 0;
    min-inline-size: 0;
  }
  dd.after {
    font-weight: 500;
  }
  .note {
    color: var(--faint);
    font-size: var(--fs-small);
  }
  .steps {
    padding-block: 10px;
    padding-inline: 12px;
    border-radius: var(--radius-control);
    background: var(--raised);
  }
  .steps summary {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: var(--fs-small);
    font-weight: 500;
    cursor: pointer;
    list-style: none;
    border-radius: var(--radius-chip);
  }
  .steps summary::-webkit-details-marker {
    display: none;
  }
  .steps summary:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  .steps-title {
    flex: 1;
    min-inline-size: 0;
  }
  .steps summary :global(.chevron) {
    flex: none;
    color: var(--muted);
    transition: rotate var(--t-fast) var(--ease);
  }
  .steps[open] summary :global(.chevron) {
    rotate: 180deg;
  }
  .steps-short {
    margin-block: 6px 0;
    margin-inline: 0;
    color: var(--muted);
    font-size: var(--fs-small);
  }
  .steps[open] .steps-short {
    display: none;
  }
  .steps ol {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-block: 8px 0;
    margin-inline: 0;
    padding: 0;
    list-style: none;
  }
  .steps li {
    display: grid;
    grid-template-columns: 14px 1fr;
    gap: 8px;
    font-size: var(--fs-small);
    color: var(--muted);
  }
  .steps li::before {
    content: '';
    inline-size: 8px;
    block-size: 8px;
    margin-block-start: 6px;
    border: 1.5px solid var(--faint);
    border-radius: 50%;
  }
  .steps li.off {
    color: var(--text);
    font-weight: 500;
  }
  .steps li.off::before {
    border-color: var(--q-fair);
  }
  .everyone {
    display: flex;
    gap: 8px;
    align-items: flex-start;
    margin: 0;
    padding-block: 8px;
    padding-inline: 10px;
    border-radius: var(--radius-control);
    background: var(--pill-warning-bg);
    color: var(--pill-warning-ink);
    font-size: var(--fs-small);
  }
  .everyone :global(svg) {
    flex: none;
    margin-block-start: 2px;
  }
  /* Pinned to the panel's foot, so Apply can't scroll out of a short window. */
  .actions {
    position: sticky;
    inset-block-end: -16px;
    z-index: 1;
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin-block-end: -16px;
    padding-block: 8px 16px;
    background: var(--surface);
  }
  .actions button {
    block-size: 36px;
  }
  button.secondary {
    background: var(--secondary);
    border-color: var(--secondary);
    font-weight: 500;
  }
  .hint {
    margin: 0;
    color: var(--muted);
    font-size: var(--fs-small);
  }
</style>
