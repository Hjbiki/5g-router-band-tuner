<script lang="ts">
  import { onDestroy, onMount, untrack } from 'svelte'
  import ChartLine from '@lucide/svelte/icons/chart-line'
  import Zap from '@lucide/svelte/icons/zap'
  import Timer from '@lucide/svelte/icons/timer'
  import Power from '@lucide/svelte/icons/power'
  import PaletteIcon from '@lucide/svelte/icons/palette'
  import Router from '@lucide/svelte/icons/router'
  import Star from '@lucide/svelte/icons/star'
  import TriangleAlert from '@lucide/svelte/icons/triangle-alert'
  import InfoTip from './ui/InfoTip.svelte'
  import { app, mergeEdits, pushToast, updateSettings } from '../lib/store.svelte'
  import { LANGUAGES, currentLanguage, resolveLanguage, setLanguage, t, type MessageKey } from '../lib/i18n'
  import { api, onAutoSpeedTest } from '../lib/backend'
  import { AUTO_TEST_WARN_MB, dataUseMb } from '../lib/history.svelte'
  import { formatDay, formatTime } from './history/TimeChart.svelte'
  import type { AutoSpeedTestStatus } from '../lib/bindings/AutoSpeedTestStatus'
  import type { SpeedTestProvider } from '../lib/bindings/SpeedTestProvider'
  import type { SpeedTestSize } from '../lib/bindings/SpeedTestSize'
  import { errorText } from '../lib/errors'
  import { formatNumber, isolate } from '../lib/format'
  import { applyTheme, DESIGNS, PALETTES, THEMES } from '../lib/theme'
  import { applyUiScale, UI_SCALES } from '../lib/uiScale'
  import { dock } from '../lib/dock'
  import { refreshCred } from '../lib/credentials.svelte'
  import type { Settings } from '../lib/bindings/Settings'
  import type { Palette } from '../lib/bindings/Palette'
  import type { ThemeMode } from '../lib/bindings/ThemeMode'

  const RANGES = {
    graceS: [30, 300],
    lossS: [20, 300],
    probationS: [60, 600],
  } as const
  type TimerKey = keyof typeof RANGES
  const TIMER_KEYS = Object.keys(RANGES) as TimerKey[]

  // Edit a copy; the stored settings only change after a successful save. `base` is the stored
  // settings the form was last seeded from: when something else changes the settings while this
  // page is open (Switch back, the band star, the speed-test page), the fields the user hasn't
  // touched follow the new values, and Save writes only the user's own edits.
  let form = $state(null) as Settings | null
  let base = $state.raw(null) as Settings | null
  $effect(() => {
    if (!app.settings) return
    const stored = $state.snapshot(app.settings) as Settings
    untrack(() => {
      if (!form || !base) {
        form = stored
        base = structuredClone(stored)
      } else if (JSON.stringify(stored) !== JSON.stringify(base)) {
        form = mergeEdits(base, $state.snapshot(form) as Settings, stored)
        base = structuredClone(stored)
      }
    })
  })

  // Theme and palette preview straight away; leaving without saving puts the stored look back.
  $effect(() => {
    if (form) applyTheme(form.theme, form.palette, form.design)
  })
  $effect(() => {
    if (form) applyUiScale(form.uiScale, form.design === 'spectrum')
  })
  onDestroy(() => {
    applyTheme(app.settings?.theme, app.settings?.palette, app.settings?.design)
    applyUiScale(app.settings?.uiScale, app.settings?.design === 'spectrum')
  })

  let saving = $state(false)
  let formError: string | null = $state(null)
  // Turning auto-undo off asks first; this is the pending question.
  let confirmingOff = $state(false)
  let confirmCancel: HTMLButtonElement | undefined = $state()
  $effect(() => {
    if (confirmingOff) confirmCancel?.focus()
  })
  const digits = $derived(app.settings?.digits ?? 'western')
  // The keychain entry is named after the address and username, so the password waits for them.
  const accountDirty = $derived(
    !!form && !!base && (form.host !== base.host || form.username !== base.username || form.driver !== base.driver),
  )

  const timerLabels: Record<TimerKey, 'settings.reattachGrace' | 'settings.lossThreshold' | 'settings.probation'> = {
    graceS: 'settings.reattachGrace',
    lossS: 'settings.lossThreshold',
    probationS: 'settings.probation',
  }
  const timerHelp: Record<TimerKey, MessageKey> = {
    graceS: 'settings.help.reattachGrace',
    lossS: 'settings.help.lossThreshold',
    probationS: 'settings.help.probation',
  }
  const themeLabels: Record<ThemeMode, MessageKey> = {
    system: 'appearance.themeSystem',
    light: 'appearance.themeLight',
    dark: 'appearance.themeDark',
  }
  const paletteLabels: Record<Palette, MessageKey> = {
    carbon: 'appearance.carbon',
    cobalt: 'appearance.cobalt',
    sandstone: 'appearance.sandstone',
  }

  function timerError(k: TimerKey): string | null {
    if (!form) return null
    const v = form.timers[k]
    const [min, max] = RANGES[k]
    if (Number.isInteger(v) && v >= min && v <= max) return null
    return t('settings.invalidRange', {
      min: isolate(formatNumber(min, { digits })),
      max: isolate(formatNumber(max, { digits })),
    })
  }
  const timersValid = $derived(TIMER_KEYS.every((k) => timerError(k) === null))
  const hostBlank = $derived(!form || form.host.trim() === '')
  const userBlank = $derived(!form || form.username.trim() === '')

  // History: the automatic tests and how long records are kept. The ranges mirror settings.rs.
  const EVERY_PRESETS: readonly number[] = [1, 2, 3, 5]
  const EVERY_RANGE = [1, 168] as const
  const KEEP_PRESETS: readonly number[] = [30, 90, 365]
  const KEEP_RANGE = [7, 3650] as const
  const KEEP_RECOMMENDED = 90
  const SIZES: readonly SpeedTestSize[] = ['light', 'full']
  const PROVIDERS: readonly SpeedTestProvider[] = ['cloudflare', 'ookla']
  // Custom stays chosen while its number happens to equal a preset.
  let everyCustomPicked = $state(false)
  let keepCustomPicked = $state(false)
  const everyChoice = $derived.by((): number | 'custom' => {
    const h = form?.history.autoSpeedTest.everyH ?? 3
    return everyCustomPicked || !EVERY_PRESETS.includes(h) ? 'custom' : h
  })
  const keepChoice = $derived.by((): number | 'custom' | 'forever' => {
    const d = form?.history.retentionDays
    if (d === null) return 'forever'
    if (d === undefined) return KEEP_RECOMMENDED
    return keepCustomPicked || !KEEP_PRESETS.includes(d) ? 'custom' : d
  })

  function rangeError(v: number | null | undefined, [min, max]: readonly [number, number]): string | null {
    if (typeof v === 'number' && Number.isInteger(v) && v >= min && v <= max) return null
    return t('settings.invalidRange', {
      min: isolate(formatNumber(min, { digits })),
      max: isolate(formatNumber(max, { digits })),
    })
  }
  const everyError = $derived(form ? rangeError(form.history.autoSpeedTest.everyH, EVERY_RANGE) : null)
  const keepError = $derived(
    form && form.history.retentionDays !== null ? rangeError(form.history.retentionDays, KEEP_RANGE) : null,
  )
  const historyValid = $derived(everyError === null && keepError === null)
  const canSave = $derived(form !== null && timersValid && historyValid && !hostBlank && !userBlank && !saving)

  // Arabic counts need their own word for 1, 2 and 3 to 10 hours.
  const everyLabel = (h: number) =>
    t(h === 1 ? 'settings.autoTest.everyOne' : h === 2 ? 'settings.autoTest.everyTwo' : 'settings.autoTest.everyFew', {
      n: formatNumber(h, { digits }),
    })

  function pickEvery(choice: number | 'custom') {
    if (!form) return
    everyCustomPicked = choice === 'custom'
    if (choice !== 'custom') form.history.autoSpeedTest.everyH = choice
  }
  function pickKeep(choice: number | 'custom' | 'forever') {
    if (!form) return
    keepCustomPicked = choice === 'custom'
    if (choice === 'forever') form.history.retentionDays = null
    else if (choice === 'custom') form.history.retentionDays ??= base?.history.retentionDays ?? KEEP_RECOMMENDED
    else form.history.retentionDays = choice
  }
  function toggleAutoTest() {
    if (!form) return
    const a = form.history.autoSpeedTest
    a.enabled = !a.enabled
    // The interval is hidden while off: a half-typed custom value mustn't block Save unseen.
    if (!a.enabled && everyError !== null) {
      a.everyH = base?.history.autoSpeedTest.everyH ?? 3
      everyCustomPicked = false
    }
  }

  const ooklaAccepted = $derived(form?.speedTest.ooklaTermsAccepted ?? false)
  const dataMb = $derived(form ? dataUseMb(form.history.autoSpeedTest, ooklaAccepted) : 0)
  const dataText = $derived(isolate(t('scan.mb', { n: formatNumber(Math.round(dataMb), { digits }) })))
  const autoDirty = $derived(
    !!form && !!base && JSON.stringify(form.history.autoSpeedTest) !== JSON.stringify(base.history.autoSpeedTest),
  )

  // The scheduler's own word on what comes next, and the history file's size.
  let autoStatus: AutoSpeedTestStatus | null = $state(null)
  let dbBytes: number | null = $state(null)
  let historyAvailable = $state(true)
  async function loadOverview() {
    try {
      const o = await api.historyOverview()
      historyAvailable = o.available
      dbBytes = o.available ? o.dbBytes : null
    } catch {
      historyAvailable = false
      dbBytes = null
    }
  }
  onMount(() => {
    let off: (() => void) | undefined
    let gone = false
    void Promise.resolve()
      .then(() => api.autoSpeedTestStatus())
      .then((s) => {
        if (!gone && s) autoStatus = s
      })
      .catch(() => {})
    void Promise.resolve()
      .then(() => onAutoSpeedTest((s) => (autoStatus = s)))
      .then((u) => {
        if (gone) u?.()
        else off = u
      })
      .catch(() => {})
    void loadOverview()
    return () => {
      gone = true
      off?.()
    }
  })

  const lang = $derived(currentLanguage())
  function when(ms: number): string {
    const sameDay = new Date(ms).toDateString() === new Date().toDateString()
    const time = formatTime(ms, lang, digits)
    return isolate(sameDay ? time : `${formatDay(ms, lang, digits)} ${time}`)
  }
  const statusLines = $derived.by((): string[] => {
    const s = autoStatus
    if (!s || !s.enabled) return []
    const lines: string[] = []
    if (s.running) lines.push(t('settings.autoTest.status.running'))
    else if (s.blockedBy)
      lines.push(t('settings.autoTest.status.blocked', { reason: t(`settings.autoTest.reason.${s.blockedBy}`) }))
    else if (s.nextSlotMs !== null) lines.push(t('settings.autoTest.status.next', { time: when(s.nextSlotMs) }))
    if (s.lastAttemptMs !== null && s.lastOutcome) {
      const time = when(s.lastAttemptMs)
      lines.push(
        s.lastOutcome === 'skipped'
          ? t('settings.autoTest.status.skipped', { time, reason: t(`events.why.${s.lastSkipReason ?? 'offline'}`) })
          : t(`settings.autoTest.status.${s.lastOutcome}`, { time }),
      )
    }
    return lines
  })
  const diskText = $derived(
    dbBytes === null
      ? null
      : isolate(
          // Below 0.1 MB, "0.0 MB" would read as nothing or as an error.
          dbBytes < 100_000
            ? t('scan.kb', { n: formatNumber(Math.max(1, Math.round(dbBytes / 1_000)), { digits }) })
            : t('scan.mb', { n: formatNumber(dbBytes / 1_000_000, { digits, fraction: dbBytes < 10_000_000 ? 1 : 0 }) }),
        ),
  )

  // Clear history asks first, like turning auto-undo off.
  let confirmingClear = $state(false)
  let clearing = $state(false)
  let clearError: string | null = $state(null)
  let clearCancel: HTMLButtonElement | undefined = $state()
  $effect(() => {
    if (confirmingClear) clearCancel?.focus()
  })
  async function clearAll() {
    if (clearing) return
    clearing = true
    clearError = null
    try {
      await api.clearHistory()
      confirmingClear = false
      pushToast(t('settings.history.cleared'))
      void loadOverview()
    } catch (e) {
      clearError = errorText(e)
    } finally {
      clearing = false
    }
  }

  // The fields the save bar names, in the order they appear on the page.
  const changed = $derived.by((): string[] => {
    if (!form || !base) return []
    const f = form
    const b = base
    const fields: [boolean, MessageKey][] = [
      [f.automation.relockPrimary !== b.automation.relockPrimary, 'automation.relock'],
      [f.automation.revertOnLoss !== b.automation.revertOnLoss, 'automation.revertOnLoss'],
      [f.cellAlerts !== b.cellAlerts, 'automation.cellAlerts'],
      [f.timers.graceS !== b.timers.graceS, 'settings.reattachGrace'],
      [f.timers.lossS !== b.timers.lossS, 'settings.lossThreshold'],
      [f.timers.probationS !== b.timers.probationS, 'settings.probation'],
      [f.history.autoSpeedTest.enabled !== b.history.autoSpeedTest.enabled, 'settings.autoTest.name'],
      [f.history.autoSpeedTest.everyH !== b.history.autoSpeedTest.everyH, 'settings.autoTest.every'],
      [f.history.autoSpeedTest.size !== b.history.autoSpeedTest.size, 'speedtest.settings.size'],
      [f.history.autoSpeedTest.provider !== b.history.autoSpeedTest.provider, 'speedtest.settings.provider'],
      [f.history.retentionDays !== b.history.retentionDays, 'settings.history.keep'],
      [f.startAtLogin !== b.startAtLogin, 'settings.startAtLogin'],
      [f.updateCheck !== b.updateCheck, 'updates.setting'],
      [f.theme !== b.theme, 'appearance.theme'],
      [f.palette !== b.palette, 'appearance.palette'],
      [f.design !== b.design, 'appearance.design'],
      [f.uiScale !== b.uiScale, 'settings.uiScale'],
      [f.language !== b.language, 'settings.language'],
      [f.digits !== b.digits, 'settings.digits'],
      [f.driver !== b.driver, 'settings.driver'],
      [f.host !== b.host, 'settings.host'],
      [f.username !== b.username, 'settings.username'],
    ]
    return fields.filter(([differs]) => differs).map(([, key]) => t(key))
  })
  const unsavedText = $derived(
    changed.length === 1
      ? t('settings.unsavedOne', { what: changed[0] })
      : t('settings.unsavedMany', {
          n: isolate(formatNumber(changed.length, { digits })),
          what: changed.join(t('settings.listSep')),
        }),
  )

  function discard() {
    if (!base) return
    form = structuredClone($state.snapshot(base) as Settings)
    formError = null
    confirmingOff = false
    everyCustomPicked = false
    keepCustomPicked = false
  }

  /** "3 min" or "45 s", for the auto-undo explanation. Falls back to the stored value while a field is invalid. */
  function duration(k: TimerKey): string {
    const typed = form?.timers[k]
    const s = timerError(k) === null && typed !== undefined ? typed : (app.settings?.timers[k] ?? RANGES[k][0])
    const text =
      s % 60 === 0
        ? t('automation.minutes', { n: formatNumber(s / 60, { digits }) })
        : t('automation.seconds', { n: formatNumber(s, { digits }) })
    return isolate(text)
  }

  const primaryBand = $derived(form?.automation.primaryBand ?? null)
  const languageSystemName = $derived(
    LANGUAGES.find((l) => l.code === resolveLanguage('system'))?.name ?? 'English',
  )

  function toggleRevertOnLoss() {
    if (!form) return
    if (form.automation.revertOnLoss) {
      confirmingOff = true
    } else {
      form.automation.revertOnLoss = true
    }
  }

  function confirmOff() {
    if (form) form.automation.revertOnLoss = false
    confirmingOff = false
  }

  async function save() {
    if (!form || !base || !canSave) return
    saving = true
    formError = null
    const from = base
    const edited = $state.snapshot(form) as Settings
    try {
      // Only the user's edits, applied to the settings as they are when the save runs.
      const next = await updateSettings((current) => {
        const merged = mergeEdits(from, edited, current)
        return { ...merged, host: merged.host.trim(), username: merged.username.trim() }
      })
      if (!next) return
      form = structuredClone(next)
      base = structuredClone(next)
      setLanguage(resolveLanguage(next.language))
      // Another account may have no saved password; say so instead of failing at Connect.
      if (next.host !== from.host || next.username !== from.username || next.driver !== from.driver) await refreshCred()
      pushToast(t('settings.saved'))
    } catch (e) {
      formError = errorText(e)
    } finally {
      saving = false
    }
  }
</script>

{#snippet toggle(id: string, checked: boolean, onclick: () => void, disabled = false)}
  <button
    type="button"
    role="switch"
    class="switch"
    aria-checked={checked}
    aria-labelledby={`${id}-name`}
    aria-describedby={`${id}-help`}
    {disabled}
    {onclick}
    ><span class="knob" aria-hidden="true"></span><span class="state" aria-hidden="true"
      >{checked ? t('settings.switchOn') : t('settings.switchOff')}</span
    ></button
  >
{/snippet}

<section class="settings" aria-labelledby="settings-title">
  <h1 id="settings-title">{t('settings.title')}</h1>
  <p class="subtitle">{t('settings.subtitle')}</p>
  {#if !form}
    <p role="status">{t('app.loading')}</p>
  {:else}
    <form
      id="settings-form"
      onsubmit={(e) => {
        e.preventDefault()
        void save()
      }}
      autocomplete="off"
    >
      <div class="column">
        <section class="card" aria-labelledby="sec-automation">
          <h2 id="sec-automation"><Zap size={18} aria-hidden="true" />{t('automation.title')}</h2>
          <p class="hint lead">{t('automation.hint')}</p>
          <div class="row">
            <div class="lbl">
              <div class="name">
                <span class="name-text" id="relock-name">
                  {t('automation.relock')}
                  {#if primaryBand !== null}
                    <span class="band">
                      <Star size={15} class="star" aria-label={t('automation.primaryStar')} />
                      <bdi dir="ltr">B{primaryBand}</bdi>
                    </span>
                  {/if}
                </span>
                <InfoTip label={t('metrics.whatIs', { name: t('automation.primaryStar') })} text={t('settings.help.relock')} />
              </div>
              <div class="hint" id="relock-help">
                {#if primaryBand !== null}
                  {t('automation.relockHelp', { band: isolate(`B${primaryBand}`) })}
                {:else}
                  {t('automation.noPrimary')}
                  <button type="button" class="link" onclick={() => (app.tab = 'bands')}
                    >{t('automation.openBands')}</button
                  >
                {/if}
              </div>
            </div>
            {@render toggle(
              'relock',
              form.automation.relockPrimary,
              () => form && (form.automation.relockPrimary = !form.automation.relockPrimary),
              primaryBand === null,
            )}
          </div>

          <div class="row">
            <div class="lbl">
              <div class="name"><span id="revert-name">{t('automation.revertOnLoss')}</span></div>
              <div class="hint" id="revert-help">
                {t('automation.revertOnLossHelp', { probation: duration('probationS'), loss: duration('lossS') })}
              </div>
            </div>
            {@render toggle('revert', form.automation.revertOnLoss, toggleRevertOnLoss)}
            {#if confirmingOff}
              <div class="confirm" role="alertdialog" aria-labelledby="revert-confirm-title" aria-describedby="revert-confirm-body">
                <div>
                  <strong id="revert-confirm-title">{t('automation.confirmOffTitle')}</strong>
                  <p id="revert-confirm-body">{t('automation.confirmOffBody')}</p>
                </div>
                <div class="actions">
                  <button type="button" bind:this={confirmCancel} onclick={() => (confirmingOff = false)}
                    >{t('app.cancel')}</button
                  >
                  <button type="button" class="danger" onclick={confirmOff}>{t('automation.confirmOff')}</button>
                </div>
              </div>
            {:else if !form.automation.revertOnLoss}
              <div class="warning" role="alert">
                <TriangleAlert size={16} aria-hidden="true" />
                <p>
                  <strong>{t('automation.offTitle')}</strong>
                  {t('automation.offBody', { revert: t('safeApply.revert') })}
                </p>
                <button type="button" class="back-on" onclick={() => form && (form.automation.revertOnLoss = true)}
                  >{t('automation.turnBackOn')}</button
                >
              </div>
            {/if}
          </div>

          <div class="row">
            <div class="lbl">
              <div class="name"><span id="cell-name">{t('automation.cellAlerts')}</span></div>
              <div class="hint" id="cell-help">{t('automation.cellAlertsHelp')}</div>
            </div>
            {@render toggle('cell', form.cellAlerts, () => form && (form.cellAlerts = !form.cellAlerts))}
          </div>
        </section>

        <section class="card" aria-labelledby="sec-connection">
          <h2 id="sec-connection"><Router size={18} aria-hidden="true" />{t('settings.routerConnection')}</h2>
          <p class="hint lead">{t('settings.connectionHint')}</p>
          {#if accountDirty}<p class="hint" data-testid="login-account-dirty">{t('login.accountDirty')}</p>{/if}
          <div class="row">
            <div class="name">
              <label for="set-driver">{t('settings.driver')}</label>
              <InfoTip label={t('metrics.whatIs', { name: t('settings.driver') })} text={t('settings.help.driver')} />
            </div>
            <select id="set-driver" class="ctl" bind:value={form.driver}>
              <option value="huawei_h122">{t('settings.driverHuawei')}</option>
              <option value="mock">{t('settings.driverMock')}</option>
            </select>
          </div>
          <div class="row">
            <label class="name" for="set-host">{t('settings.host')}</label>
            <input id="set-host" class="ctl" type="text" dir="ltr" bind:value={form.host} spellcheck="false" autocomplete="off" />
          </div>
          <div class="row">
            <label class="name" for="set-user">{t('settings.username')}</label>
            <input id="set-user" class="ctl" type="text" dir="ltr" bind:value={form.username} spellcheck="false" autocomplete="off" />
          </div>
        </section>

        <section class="card" aria-labelledby="sec-startup">
          <h2 id="sec-startup"><Power size={18} aria-hidden="true" />{t('settings.startup')}</h2>
          <div class="row">
            <div class="lbl">
              <div class="name"><span id="login-name">{t('settings.startAtLogin')}</span></div>
              <div class="hint" id="login-help">{t('settings.startupHint')}</div>
            </div>
            {@render toggle('login', form.startAtLogin, () => form && (form.startAtLogin = !form.startAtLogin))}
          </div>
          <div class="row">
            <div class="lbl">
              <div class="name"><span id="updates-name">{t('updates.setting')}</span></div>
              <div class="hint" id="updates-help">{t('updates.questionHint')}</div>
            </div>
            {@render toggle('updates', form.updateCheck === true, () => form && (form.updateCheck = form.updateCheck !== true))}
          </div>
        </section>
      </div>

      <div class="column">
        <section class="card" aria-labelledby="sec-look">
          <h2 id="sec-look"><PaletteIcon size={18} aria-hidden="true" />{t('appearance.title')}</h2>
          <p class="hint lead">{t('appearance.hint')}</p>
          <fieldset class="row" data-testid="design-choice">
            <legend class="sr-only">{t('appearance.design')}</legend>
            <div class="lbl">
              <div class="name" aria-hidden="true">{t('appearance.design')}</div>
              <div class="hint">{t('appearance.designHint')}</div>
            </div>
            <div class="seg">
              {#each DESIGNS as d (d)}
                <label class:on={form.design === d}>
                  <input class="sr-only" type="radio" name="design" value={d} bind:group={form.design} />
                  {t(`appearance.designs.${d}`)}
                </label>
              {/each}
            </div>
          </fieldset>
          <fieldset class="row">
            <legend class="sr-only">{t('appearance.theme')}</legend>
            <div class="name" aria-hidden="true">{t('appearance.theme')}</div>
            <div class="seg">
              {#each THEMES as m (m)}
                <label class:on={form.theme === m}>
                  <input class="sr-only" type="radio" name="theme" value={m} bind:group={form.theme} />
                  {t(themeLabels[m])}
                </label>
              {/each}
            </div>
          </fieldset>
          <fieldset class="row">
            <legend class="sr-only">{t('appearance.palette')}</legend>
            <div class="name" aria-hidden="true">{t('appearance.palette')}</div>
            <div class="palettes">
              {#each PALETTES as p (p)}
                <label class="palette" class:on={form.palette === p} title={t(paletteLabels[p])}>
                  <input class="sr-only" type="radio" name="palette" value={p} bind:group={form.palette} />
                  <span class="swatches swatch-{p}" aria-hidden="true"><i></i><i></i><i></i></span>
                  <span class="sr-only">{t(paletteLabels[p])}</span>
                </label>
              {/each}
            </div>
          </fieldset>
          <fieldset class="row stacked">
            <legend class="sr-only">{t('settings.uiScale')}</legend>
            <div class="lbl">
              <div class="name" aria-hidden="true">{t('settings.uiScale')}</div>
              <div class="hint">
                {t('settings.uiScaleHint', {
                  bigger: isolate('Ctrl + ='),
                  smaller: isolate('Ctrl + −'),
                  reset: isolate('Ctrl + 0'),
                })}
              </div>
            </div>
            <div class="seg num" dir="ltr">
              {#each UI_SCALES as n (n)}
                <label class:on={form.uiScale === n}>
                  <input class="sr-only" type="radio" name="ui-scale" value={n} bind:group={form.uiScale} />
                  {formatNumber(n, { digits })}%
                </label>
              {/each}
            </div>
          </fieldset>
          <div class="row">
            <label class="name" for="set-language">{t('settings.language')}</label>
            <select id="set-language" class="ctl" bind:value={form.language}>
              <option value="system">{t('settings.languageSystem', { language: languageSystemName })}</option>
              {#each LANGUAGES as l (l.code)}
                <option value={l.code}>{l.name}</option>
              {/each}
            </select>
          </div>
          <div class="row">
            <label class="name" for="set-digits">{t('settings.digits')}</label>
            <select id="set-digits" class="ctl" bind:value={form.digits}>
              <option value="western">{t('settings.digitsWestern')}</option>
              <option value="arabicIndic">{t('settings.digitsArabicIndic')}</option>
            </select>
          </div>
        </section>

        <section class="card" aria-labelledby="sec-timers">
          <h2 id="sec-timers"><Timer size={18} aria-hidden="true" />{t('settings.timers')}</h2>
          <p class="hint lead">{t('settings.timersHint')}</p>
          {#each TIMER_KEYS as k (k)}
            {@const err = timerError(k)}
            <div class="row">
              <div class="lbl">
                <div class="name">
                  <label for={`timer-${k}`}>{t(timerLabels[k])}</label>
                  <InfoTip label={t('metrics.whatIs', { name: t(timerLabels[k]) })} text={t(timerHelp[k])} />
                </div>
                <div class="hint num">
                  {t('settings.timerRange', {
                    min: isolate(formatNumber(RANGES[k][0], { digits })),
                    max: isolate(formatNumber(RANGES[k][1], { digits })),
                  })}
                </div>
              </div>
              <input
                id={`timer-${k}`}
                class="short"
                type="number"
                dir="ltr"
                min={RANGES[k][0]}
                max={RANGES[k][1]}
                step="1"
                bind:value={form.timers[k]}
                aria-invalid={err !== null}
                aria-describedby={err ? `err-${k}` : undefined}
              />
              {#if err}<p id={`err-${k}`} class="err wide" role="alert">{err}</p>{/if}
            </div>
          {/each}
        </section>

        <section class="card" aria-labelledby="sec-history">
          <h2 id="sec-history"><ChartLine size={18} aria-hidden="true" />{t('settings.history.title')}</h2>
          <p class="hint lead">{t('settings.history.hint')}</p>
          <div class="row">
            <div class="lbl">
              <div class="name"><span id="autotest-name">{t('settings.autoTest.name')}</span></div>
              <div class="hint" id="autotest-help">{t('settings.autoTest.help')}</div>
            </div>
            {@render toggle('autotest', form.history.autoSpeedTest.enabled, toggleAutoTest)}
            <div class="wide status" role="status" data-testid="auto-status">
              {#if autoDirty}
                <p>{t('settings.autoTest.saveFirst')}</p>
              {:else}
                {#each statusLines as line, i (i)}<p>{line}</p>{/each}
              {/if}
            </div>
          </div>

          {#if form.history.autoSpeedTest.enabled}
            <fieldset class="row stacked">
              <legend class="sr-only">{t('settings.autoTest.every')}</legend>
              <div class="name" aria-hidden="true">{t('settings.autoTest.every')}</div>
              <div class="seg wrap">
                {#each EVERY_PRESETS as h (h)}
                  <label class:on={everyChoice === h}>
                    <input class="sr-only" type="radio" name="auto-every" checked={everyChoice === h} onchange={() => pickEvery(h)} />
                    {everyLabel(h)}
                  </label>
                {/each}
                <label class:on={everyChoice === 'custom'}>
                  <input
                    class="sr-only"
                    type="radio"
                    name="auto-every"
                    checked={everyChoice === 'custom'}
                    onchange={() => pickEvery('custom')}
                  />
                  {t('settings.autoTest.custom')}
                </label>
              </div>
              {#if everyChoice === 'custom'}
                <div class="custom">
                  <label for="auto-every-custom">{t('settings.autoTest.customHours')}</label>
                  <input
                    id="auto-every-custom"
                    class="short"
                    type="number"
                    dir="ltr"
                    min={EVERY_RANGE[0]}
                    max={EVERY_RANGE[1]}
                    step="1"
                    value={form.history.autoSpeedTest.everyH}
                    oninput={(e) => form && (form.history.autoSpeedTest.everyH = e.currentTarget.valueAsNumber)}
                    aria-invalid={everyError !== null}
                    aria-describedby="auto-every-range"
                  />
                  <span class="hint num" id="auto-every-range">
                    {t('settings.autoTest.customRange', {
                      min: isolate(formatNumber(EVERY_RANGE[0], { digits })),
                      max: isolate(formatNumber(EVERY_RANGE[1], { digits })),
                    })}
                  </span>
                  {#if everyError}<p class="err wide" role="alert">{everyError}</p>{/if}
                </div>
              {/if}
            </fieldset>

            <fieldset class="row stacked">
              <legend class="sr-only">{t('speedtest.settings.size')}</legend>
              <div class="name" aria-hidden="true">{t('speedtest.settings.size')}</div>
              <div class="seg wrap">
                {#each SIZES as z (z)}
                  <label class:on={form.history.autoSpeedTest.size === z}>
                    <input class="sr-only" type="radio" name="auto-size" value={z} bind:group={form.history.autoSpeedTest.size} />
                    {t(`speedtest.settings.${z}`)}
                  </label>
                {/each}
              </div>
              {#if form.history.autoSpeedTest.provider === 'ookla' && ooklaAccepted}
                <p class="hint">{t('speedtest.settings.ooklaSize')}</p>
              {/if}
            </fieldset>

            <fieldset class="row stacked">
              <legend class="sr-only">{t('speedtest.settings.provider')}</legend>
              <div class="name" aria-hidden="true">{t('speedtest.settings.provider')}</div>
              <div class="seg wrap">
                {#each PROVIDERS as pv (pv)}
                  {@const locked = pv === 'ookla' && !ooklaAccepted}
                  <label class:on={form.history.autoSpeedTest.provider === pv} class:locked>
                    <input
                      class="sr-only"
                      type="radio"
                      name="auto-provider"
                      value={pv}
                      bind:group={form.history.autoSpeedTest.provider}
                      disabled={locked}
                      aria-describedby={locked ? 'auto-ookla-terms' : undefined}
                    />
                    {t(`speedtest.settings.${pv}`)}
                  </label>
                {/each}
              </div>
              {#if !ooklaAccepted}<p class="hint" id="auto-ookla-terms">{t('settings.autoTest.ooklaTerms')}</p>{/if}
            </fieldset>

            <div class="row stacked">
              {#if dataMb > AUTO_TEST_WARN_MB}
                <div class="warning" data-testid="auto-data-warning">
                  <TriangleAlert size={16} aria-hidden="true" />
                  <p><strong>{t('settings.autoTest.dataUse', { mb: dataText })}</strong> {t('settings.autoTest.dataUseHigh')}</p>
                </div>
              {:else}
                <p class="hint" data-testid="auto-data-use">{t('settings.autoTest.dataUse', { mb: dataText })}</p>
              {/if}
              <p class="hint">{t('settings.autoTest.skipped')}</p>
            </div>
          {/if}

          <fieldset class="row stacked">
            <legend class="sr-only">{t('settings.history.keep')}</legend>
            <div class="lbl">
              <div class="name" aria-hidden="true">{t('settings.history.keep')}</div>
              <div class="hint">{t('settings.history.keepHint')}</div>
            </div>
            <div class="seg wrap">
              {#each KEEP_PRESETS as d (d)}
                {@const label = t('settings.history.days', { n: formatNumber(d, { digits }) })}
                <label class:on={keepChoice === d}>
                  <input class="sr-only" type="radio" name="keep" checked={keepChoice === d} onchange={() => pickKeep(d)} />
                  {d === KEEP_RECOMMENDED ? t('settings.history.recommended', { label }) : label}
                </label>
              {/each}
              <label class:on={keepChoice === 'custom'}>
                <input class="sr-only" type="radio" name="keep" checked={keepChoice === 'custom'} onchange={() => pickKeep('custom')} />
                {t('settings.history.custom')}
              </label>
              <label class:on={keepChoice === 'forever'}>
                <input class="sr-only" type="radio" name="keep" checked={keepChoice === 'forever'} onchange={() => pickKeep('forever')} />
                {t('settings.history.forever')}
              </label>
            </div>
            {#if keepChoice === 'custom'}
              <div class="custom">
                <label for="keep-custom">{t('settings.history.customDays')}</label>
                <input
                  id="keep-custom"
                  class="short"
                  type="number"
                  dir="ltr"
                  min={KEEP_RANGE[0]}
                  max={KEEP_RANGE[1]}
                  step="1"
                  value={form.history.retentionDays}
                  oninput={(e) => form && (form.history.retentionDays = e.currentTarget.valueAsNumber)}
                  aria-invalid={keepError !== null}
                  aria-describedby="keep-custom-range"
                />
                <span class="hint num" id="keep-custom-range">
                  {t('settings.history.customRange', {
                    min: isolate(formatNumber(KEEP_RANGE[0], { digits })),
                    max: isolate(formatNumber(KEEP_RANGE[1], { digits })),
                  })}
                </span>
                {#if keepError}<p class="err wide" role="alert">{keepError}</p>{/if}
              </div>
            {/if}
          </fieldset>

          <div class="row">
            <p class="hint" data-testid="history-disk">
              {#if diskText}
                {t('settings.history.disk', { size: diskText })}
              {:else if !historyAvailable}
                {t('errors.historyUnavailable')}
              {/if}
            </p>
            <button
              type="button"
              class="secondary"
              onclick={() => (confirmingClear = true)}
              disabled={!historyAvailable || clearing || confirmingClear}>{t('settings.history.clear')}</button
            >
            {#if confirmingClear}
              <div class="confirm" role="alertdialog" aria-labelledby="clear-confirm-title" aria-describedby="clear-confirm-body">
                <div>
                  <strong id="clear-confirm-title">{t('settings.history.clearTitle')}</strong>
                  <p id="clear-confirm-body">{t('settings.history.clearBody')}</p>
                </div>
                <div class="actions">
                  <button type="button" bind:this={clearCancel} onclick={() => (confirmingClear = false)} disabled={clearing}
                    >{t('app.cancel')}</button
                  >
                  <button type="button" class="danger" onclick={clearAll} disabled={clearing}
                    >{clearing ? t('settings.history.clearing') : t('settings.history.clearConfirm')}</button
                  >
                </div>
              </div>
            {/if}
            {#if clearError}<p class="err wide" role="alert">{clearError}</p>{/if}
          </div>
        </section>
      </div>
    </form>

    {#if changed.length > 0 || formError}
      <div class="savebar" role="region" aria-label={t('settings.unsaved')} use:dock>
        <div class="savebar-inner">
          {#if formError}
            <p class="err" role="alert">{formError}</p>
          {:else}
            <span class="dot" aria-hidden="true"></span>
            <p data-testid="unsaved">{unsavedText}</p>
          {/if}
          <button type="button" class="secondary" onclick={discard}>{t('settings.discard')}</button>
          <button type="submit" form="settings-form" class="primary" disabled={!canSave}>{t('settings.saveChanges')}</button>
        </div>
      </div>
    {/if}
  {/if}
</section>

<style>
  h1 {
    font-size: var(--fs-title);
    line-height: 32px;
    margin: 0;
  }
  .subtitle {
    margin-block: 2px var(--gap);
    margin-inline: 0;
    color: var(--muted);
    max-inline-size: var(--measure);
  }
  form {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: var(--gap);    align-items: start;
  }
  @media (max-width: 1120px) {
    form {
      grid-template-columns: minmax(0, 1fr);
    }
  }
  .column {
    display: flex;
    flex-direction: column;
    gap: var(--gap);
    min-inline-size: 0;
  }
  .card {
    display: flex;
    flex-direction: column;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius-card);
    padding-block: var(--card-pad-block) 6px;
    padding-inline: var(--card-pad-inline);
    min-inline-size: 0;
  }
  .hint {
    margin: 0;
    color: var(--muted);
    font-size: var(--fs-small);
    line-height: 20px;
  }
  .lead {
    margin-block: 2px 8px;
  }
  /* Every setting is one line: its name and help at the start, its control at the end. */
  .row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    align-items: center;
    gap: 6px 16px;
    margin: 0;
    padding-block: 12px;
    padding-inline: 0;
    border: 0;
    border-block-start: 1px solid var(--rule);
    min-inline-size: 0;
  }
  .card > h2 + .row {
    border-block-start: 0;
  }
  /* A long hint gets the full width, with its control on the line below. */
  .row.stacked {
    grid-template-columns: minmax(0, 1fr);
  }
  .row.stacked .seg {
    justify-self: start;
  }
  .lbl {
    min-inline-size: 0;
  }
  .name {
    font-weight: 500;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 2px 6px;
  }
  .name-text {
    display: inline-flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
  }
  .wide {
    grid-column: 1 / -1;
  }
  .ctl {
    inline-size: 240px;
    max-inline-size: 100%;
    block-size: var(--control-height);
    padding-block: 0;
  }
  .short {
    inline-size: 110px;
    block-size: var(--control-height);
    padding-block: 0;
  }
  .band {
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }
  .band :global(.star) {
    color: var(--q-fair);
    fill: var(--q-fair);
  }

  /* On = green track with the knob at the end and the word "On"; off = grey with "Off".
     The same in every palette and theme, and mirrored in RTL. */
  .switch {
    all: unset;
    position: relative;
    display: inline-flex;
    align-items: center;
    gap: 8px;
    cursor: pointer;
    margin-block-start: 1px;
  }
  .switch::before {
    content: '';
    inline-size: 36px;
    block-size: 20px;
    border-radius: 10px;
    background: var(--switch-off);
    flex: none;
    transition: background-color var(--t-fast) var(--ease);
  }
  .knob {
    position: absolute;
    inset-block-start: 3px;
    inset-inline-start: 3px;
    inline-size: 14px;
    block-size: 14px;
    border-radius: 7px;
    background: #ffffff;
    transition: inset-inline-start 120ms ease;
  }
  .state {
    min-inline-size: 3em;
    font-size: var(--fs-small);
    font-weight: 600;
    color: var(--muted);
  }
  .switch[aria-checked='true']::before {
    background: var(--switch-on);
  }
  .switch[aria-checked='true'] .knob {
    inset-inline-start: 19px;
  }
  .switch[aria-checked='true'] .state {
    color: var(--switch-on-text);
  }
  @media (prefers-reduced-motion: reduce) {
    .knob {
      transition: none;
    }
  }
  .switch:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .switch:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }

  .warning,
  .confirm {
    grid-column: 1 / -1;
    margin-block-start: 6px;
    border-radius: var(--radius-control);
    padding-block: 10px;
    padding-inline: 12px;
    display: flex;
    gap: 10px;
    align-items: flex-start;
    font-size: var(--fs-small);
    line-height: 19px;
  }
  .warning {
    background: var(--pill-negative-bg);
    color: var(--pill-negative-ink);
  }
  .warning :global(svg) {
    flex: none;
    margin-block-start: 2px;
  }
  .warning p {
    flex: 1;
    margin: 0;
  }
  .warning strong {
    font-weight: 500;
  }
  .back-on {
    flex: none;
    border: 0;
    background: var(--pill-negative-ink);
    color: var(--pill-negative-bg);
    font-size: var(--fs-small);
    font-weight: 500;
    padding-block: 4px;
    padding-inline: 10px;
  }
  .confirm {
    background: var(--raised);
    border: 1px solid var(--border);
    flex-wrap: wrap;
    justify-content: space-between;
  }
  .confirm p {
    margin: 2px 0 0;
    color: var(--muted);
  }
  .confirm strong {
    font-weight: 500;
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }

  .seg {
    display: inline-flex;
    background: var(--secondary);
    border-radius: var(--radius-control);
    padding: 2px;
    gap: 2px;
  }
  .seg label {
    block-size: 28px;
    padding-inline: 12px;
    border-radius: 5px;
    font-size: var(--fs-small);
    display: inline-flex;
    align-items: center;
    color: var(--muted);
    cursor: pointer;
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
  .palettes {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  .palette {
    display: inline-flex;
    align-items: center;
    block-size: var(--control-height);
    padding-inline: 6px;
    border: 1px solid var(--control);
    border-radius: var(--radius-control);
    cursor: pointer;
  }
  .palette.on {
    border-color: var(--accent);
    outline: 1px solid var(--accent);
  }
  .seg label:has(input:focus-visible),
  .palette:has(input:focus-visible) {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  .swatches {
    display: inline-flex;
    gap: 2px;
  }
  .swatches i {
    display: block;
    inline-size: 12px;
    block-size: 18px;
    border-radius: 3px;
    border: 1px solid var(--border);
  }
  .swatch-carbon i:nth-child(1) {
    background: var(--swatch-carbon-ground);
  }
  .swatch-carbon i:nth-child(2) {
    background: var(--swatch-carbon-primary);
  }
  .swatch-carbon i:nth-child(3) {
    background: var(--swatch-carbon-accent);
  }
  .swatch-cobalt i:nth-child(1) {
    background: var(--swatch-cobalt-ground);
  }
  .swatch-cobalt i:nth-child(2) {
    background: var(--swatch-cobalt-primary);
  }
  .swatch-cobalt i:nth-child(3) {
    background: var(--swatch-cobalt-accent);
  }
  .swatch-sandstone i:nth-child(1) {
    background: var(--swatch-sandstone-ground);
  }
  .swatch-sandstone i:nth-child(2) {
    background: var(--swatch-sandstone-primary);
  }
  .swatch-sandstone i:nth-child(3) {
    background: var(--swatch-sandstone-accent);
  }

  .status p {
    margin: 0;
    color: var(--muted);
    font-size: var(--fs-small);
    line-height: 20px;
  }
  .status:empty {
    display: none;
  }
  .seg.wrap {
    flex-wrap: wrap;
  }
  .seg label.locked {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .custom {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px 12px;
  }
  .custom label {
    font-weight: 500;
  }

  .link {
    all: unset;
    color: var(--text);
    text-decoration: underline;
    cursor: pointer;
  }
  .link:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  .err {
    margin: 0;
    color: var(--q-poor);
    font-weight: 600;
  }

  /* Docked under the scrolling page by App, so it never covers a setting. */
  .savebar {
    border-block-start: 1px solid var(--border);
    background: var(--surface);
    padding-block: 12px;
    padding-inline: var(--page-gutter);
  }
  .savebar-inner {
    max-inline-size: var(--content-max);
    margin-inline: auto;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px 12px;
  }
  .savebar p {
    flex: 1;
    min-inline-size: 200px;
    margin: 0;
  }
  .dot {
    inline-size: 8px;
    block-size: 8px;
    border-radius: 4px;
    background: var(--accent);
    flex: none;
  }
  .savebar button {
    min-block-size: var(--control-height);
    padding-inline: 16px;
    font-weight: 500;
  }
  .savebar .secondary {
    background: var(--secondary);
    border-color: var(--secondary);
  }

  /* Spectrum fills the width: three columns of cards, each as tall as its content. */
  @media (min-width: 1300px) {
    :global(:root[data-design='spectrum']) form {
      display: block;
      columns: 3;
      column-gap: var(--gap);
    }
    :global(:root[data-design='spectrum']) .column {
      display: contents;
    }
    :global(:root[data-design='spectrum']) form .card {
      break-inside: avoid;
      margin-block-end: var(--gap);
    }
  }
</style>
