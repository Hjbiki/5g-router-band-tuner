<script lang="ts">
  // The Auto-scan page's body, in three steps: set it up, follow it, and pick from the ranked results.
  import ArrowDown from '@lucide/svelte/icons/arrow-down'
  import ArrowUp from '@lucide/svelte/icons/arrow-up'
  import CircleStop from '@lucide/svelte/icons/circle-stop'
  import Play from '@lucide/svelte/icons/play'
  import Check from '@lucide/svelte/icons/check'
  import ChevronDown from '@lucide/svelte/icons/chevron-down'
  import X from '@lucide/svelte/icons/x'
  import TriangleAlert from '@lucide/svelte/icons/triangle-alert'
  import Plus from '@lucide/svelte/icons/plus'
  import { api } from '../lib/backend'
  import { app, bandEdit, pushToast } from '../lib/store.svelte'
  import { t, type MessageKey } from '../lib/i18n'
  import { errorText } from '../lib/errors'
  import { formatNumber, isolate } from '../lib/format'
  import { bandsToHex, duplexOf, groupBands, isDownlinkOnly, supportedBands, type Duplex } from '../lib/bands'
  import { fiveGReason } from '../lib/diagnosis'
  import { mbps, ms, latLevel, speed, whenLabel } from '../lib/speedtest.svelte'
  import type { BandTone } from '../lib/bandHistory'
  import type { ScanMode } from '../lib/bindings/ScanMode'
  import type { ScanOutcome } from '../lib/bindings/ScanOutcome'
  import type { ScanResult } from '../lib/bindings/ScanResult'
  import type { SpeedTestSettings } from '../lib/bindings/SpeedTestSettings'
  import {
    MAX_CUSTOM_COMBOS,
    SCAN_PRIORITIES,
    SCAN_REATTACH_S,
    SCAN_REPEATS,
    busyPingOf,
    defaultRepeats,
    earlierTests,
    estimateScan,
    rankScan,
    sameCombo,
    scan,
    scanActive,
    scanBands,
    suggestCombos,
    summarizeScan,
    swingsWidely,
    workedBands,
    type ScanPriority,
  } from '../lib/scan.svelte'
  import InfoTip from './ui/InfoTip.svelte'
  import BandTags from './ui/BandTags.svelte'
  import BandToneMark from './ui/BandToneMark.svelte'
  import DuplexHeading from './ui/DuplexHeading.svelte'
  import { setRange } from '../lib/history.svelte'

  let {
    supported,
    tones,
    blocked = null,
  }: { supported: number[]; tones: Map<number, BandTone>; blocked?: MessageKey | null } = $props()

  const digits = $derived(app.settings?.digits ?? 'western')
  const cfg = $derived<SpeedTestSettings>(
    app.settings?.speedTest ?? { provider: 'cloudflare', size: 'light', ooklaTermsAccepted: false },
  )
  const file = $derived(scan.file)
  const running = $derived(scanActive())
  const num = (n: number) => isolate(formatNumber(n, { digits }))

  // --- Setup. Bands that gave no service last time start unticked: they cost the longest wait.
  let mode = $state<ScanMode>('quick')
  // Follows the mode until the user picks a number.
  let pickedRepeats: number | null = $state(null)
  const repeats = $derived(pickedRepeats ?? defaultRepeats(mode))
  let unticked = $state(new Set<number>())
  let touched = $state(new Set<number>())
  const isOn = (b: number) => (touched.has(b) ? !unticked.has(b) : tones.get(b) !== 'noService')
  const singles = $derived(supported.filter((b) => !isDownlinkOnly(b)))
  const addOns = $derived(supported.filter(isDownlinkOnly))
  const chosen = $derived(singles.filter(isOn))
  const spectrum = $derived(app.settings?.design === 'spectrum')
  // Only Thorough has combinations to add them to.
  const chosenAddOns = $derived(mode === 'thorough' ? addOns.filter(isOn) : [])
  function toggle(b: number) {
    const on = isOn(b)
    touched = new Set(touched).add(b)
    const next = new Set(unticked)
    if (on) next.add(b)
    else next.delete(b)
    unticked = next
  }
  const groups = $derived(groupBands(supported))
  const GROUPS: readonly Duplex[] = ['fdd', 'tdd', 'sdl']
  const KIND: Record<Duplex, string> = { fdd: 'FDD', tdd: 'TDD', sdl: 'SDL' }
  // Downlink-only picks only count in Thorough, which has combinations to add them to.
  const pickDisabled = (d: Duplex) => d === 'sdl' && mode !== 'thorough'
  /** Ticks or unticks every band of one group, and nothing else. */
  function setGroup(list: number[], on: boolean) {
    touched = new Set([...touched, ...list])
    const next = new Set(unticked)
    for (const b of list) {
      if (on) next.delete(b)
      else next.add(b)
    }
    unticked = next
  }

  // --- Combinations the user asks for, tried after the single bands.
  const currentHex = $derived.by((): string | null => {
    const hex = app.snapshot?.config?.lteBands
    return hex ? bandsToHex(supportedBands(hex)) : null
  })
  let customCombos = $state<number[][]>([])
  const comboMasks = $derived(customCombos.map(bandsToHex))
  const worked = $derived(workedBands(file, tones, supported))
  const suggestions = $derived(suggestCombos(worked, currentHex))
  const isAdded = (combo: number[]) => customCombos.some((c) => sameCombo(c, combo))
  function toggleCombo(combo: number[]) {
    if (isAdded(combo)) customCombos = customCombos.filter((c) => !sameCombo(c, combo))
    else if (customCombos.length < MAX_CUSTOM_COMBOS) customCombos = [...customCombos, combo]
  }
  let draftCombo = $state<number[]>([])
  function toggleDraft(b: number) {
    draftCombo = draftCombo.includes(b) ? draftCombo.filter((x) => x !== b) : [...draftCombo, b].sort((x, y) => x - y)
  }
  /** Why the draft can't be added yet, or null when it can. */
  const comboProblem = $derived.by((): MessageKey | null => {
    if (draftCombo.length < 2) return 'scan.combos.needTwo'
    if (draftCombo.every(isDownlinkOnly)) return 'scan.combos.needUplink'
    if (currentHex !== null && bandsToHex(draftCombo) === currentHex) return 'scan.combos.isCurrent'
    if (isAdded(draftCombo)) return 'scan.combos.duplicate'
    if (customCombos.length >= MAX_CUSTOM_COMBOS) return 'scan.combos.full'
    return null
  })
  const canAddCombo = $derived(comboProblem === null)
  function addDraft() {
    if (!canAddCombo) return
    customCombos = [...customCombos, draftCombo]
    draftCombo = []
  }
  const removeCombo = (i: number) => (customCombos = customCombos.filter((_, j) => j !== i))

  const estimate = $derived(
    estimateScan(mode, chosen.length, cfg, repeats, chosenAddOns.length, customCombos.length),
  )
  const dataText = $derived(
    estimate.megabytes >= 1000
      ? t('scan.gb', { n: isolate(formatNumber(estimate.megabytes / 1000, { digits, fraction: 1 })) })
      : t('scan.mb', { n: num(estimate.megabytes) }),
  )

  const warningText = $derived(
    t('scan.warning', {
      grace: t('automation.seconds', {
        n: isolate(formatNumber(Math.min(app.settings?.timers.graceS ?? SCAN_REATTACH_S, SCAN_REATTACH_S), { digits })),
      }),
    }),
  )
  // The first sentence says what it costs you, so it's the one that gets the weight.
  const warningLead = $derived(warningText.split(/(?<=\.)\s/)[0])
  const warningRest = $derived(warningText.slice(warningLead.length).trim())

  // --- Steps. A running scan always shows step 2, and a finished one opens on step 3.
  type Step = 'setup' | 'scanning' | 'results'
  const STEPS: readonly Step[] = ['setup', 'scanning', 'results']
  let opened: Step | null = $state(null)
  const step = $derived.by((): Step => {
    if (!file) return 'setup'
    if (running) return 'scanning'
    return opened ?? 'results'
  })
  const stepAt = $derived(STEPS.indexOf(step))
  const stepOpen = (s: Step) => (s === 'setup' ? !running : s === 'scanning' ? file !== null : file !== null && !running)

  let busy = $state(false)
  async function act(work: () => Promise<unknown>) {
    busy = true
    try {
      await work()
    } catch (e) {
      pushToast(errorText(e), 'error')
    } finally {
      busy = false
    }
  }
  const start = () =>
    act(async () => {
      await api.startScan(
        mode,
        [...$state.snapshot(chosen), ...$state.snapshot(chosenAddOns)] as number[],
        repeats,
        $state.snapshot(customCombos) as number[][],
      )
      opened = null
    })
  /** The same bands, mode and tests per config, from the bands the router has now. */
  const restart = () =>
    act(async () => {
      if (file)
        await api.startScan(
          file.mode,
          scanBands($state.snapshot(file.queue) as string[], $state.snapshot(file.addOnBands) as number[]),
          file.repeats,
          (file.customSets ?? []).map(supportedBands),
        )
      opened = null
    })

  // --- Running
  const position = $derived(file ? Math.min(file.next + 1, file.queue.length) : 0)
  const currentBands = $derived(file && file.next < file.queue.length ? file.queue[file.next] : null)
  const stepKey = $derived.by((): MessageKey => {
    if (file?.status === 'returning') return 'scan.step.returning'
    return `scan.step.${file?.step ?? 'applying'}` as MessageKey
  })
  const fraction = $derived(file && file.queue.length ? file.next / file.queue.length : 0)
  const upNext = $derived(file ? file.queue.slice(file.next + 1) : [])
  const STAGES = [
    ['idlePing', 'speedtest.phase.idlePing'],
    ['download', 'speedtest.phase.download'],
    ['upload', 'speedtest.phase.upload'],
  ] as const

  // --- Results
  let priority: ScanPriority = $state('balanced')
  const ranking = $derived(file ? rankScan(file, speed.history, priority, cfg.provider, Date.now()) : { rows: [], pick: null })
  const pick = $derived(ranking.pick)
  const pickIsStart = $derived(file !== null && pick !== null && sameBands(pick.bands, file.start.lteBands))
  const pickToday = $derived(ranking.rows.find((r) => r.bands === pick?.bands)?.result ?? null)
  const summary = $derived(summarizeScan(ranking.rows.flatMap((r) => (r.result ? [r.result] : [])), pick))
  const startResult = $derived(file?.results.find((r) => r.isStart) ?? null)
  const allBands = $derived(supported.length)
  // Downlink-only bands never get a turn alone, so they don't count as not tried.
  const triable = $derived(singles.length)
  const tableRows = $derived(ranking.rows.filter((r) => r.result?.outcome !== 'noService'))
  // Without earlier tests the average is today's numbers, so a second set of columns says nothing.
  const showAverage = $derived(tableRows.some((r) => (r.numbers?.earlier ?? 0) > 0))
  // An automatic test folded into the numbers ran on its own schedule, so say it's in there.
  const includesAuto = $derived(
    file !== null &&
      ranking.rows.some(
        (r) =>
          (r.numbers?.earlier ?? 0) > 0 &&
          earlierTests(r.bands, speed.history, cfg.provider, file.startedMs, Date.now()).some((x) => x.source === 'auto'),
      ),
  )

  /** Speed by band set on the History page, over the last week. */
  function seeBandSets() {
    app.historyFocus = 'bandSets'
    void setRange({ kind: '7d' })
    app.tab = 'history'
  }

  function bandsLabel(hex: string): string {
    const list = supportedBands(hex)
    if (allBands > 0 && list.length >= allBands) return t('bands.allSupported')
    return list.map((b) => `B${b}`).join(' + ')
  }
  function sameBands(a: string, b: string): boolean {
    return BigInt(`0x${a}`) === BigInt(`0x${b}`)
  }
  /** "B3 alone", since a lone band in a sentence reads like one of several. */
  function namedBands(hex: string): string {
    const label = bandsLabel(hex)
    return supportedBands(hex).length === 1 ? t('scan.summary.alone', { band: isolate(label) }) : isolate(label)
  }
  const figures = (r: ScanResult) =>
    t('scan.timeline.figures', {
      down: isolate(mbps(r.downloadMbps, digits)),
      up: isolate(mbps(r.uploadMbps, digits)),
      fiveG: r.got5g ? isolate(r.nrBand !== null ? `5G n${r.nrBand}` : '5G') : t('scan.no5g'),
    })
  // How much the pick beats the bands you have now, from the same numbers the table ranks on.
  const gainPercent = $derived.by((): number | null => {
    if (!file || !pick || pickIsStart || pick.downloadMbps === null) return null
    const now = ranking.rows.find((r) => sameBands(r.bands, file!.start.lteBands))?.numbers?.downloadMbps ?? null
    if (now === null || now <= 0) return null
    const pct = Math.round(((pick.downloadMbps - now) / now) * 100)
    return pct >= 5 ? pct : null
  })
  const nameList = (rows: ScanResult[]) => rows.map((r) => namedBands(r.bands)).join(t('bands.listSep'))
  const why = $derived.by(() => {
    if (!pick) return ''
    if (pick.got5g && startResult?.outcome === 'measured' && !startResult.got5g) return t('scan.summary.why5g')
    return t('scan.summary.whyRank', { rank: t(PRIORITY[priority]) })
  })

  const OUTCOME: Record<ScanOutcome, MessageKey> = {
    measured: 'scan.outcome.measured',
    testFailed: 'scan.outcome.testFailed',
    noService: 'scan.outcome.noService',
    failed: 'scan.outcome.failed',
  }
  const PRIORITY: Record<ScanPriority, MessageKey> = {
    balanced: 'scan.priority.balanced',
    download: 'scan.priority.download',
    upload: 'scan.priority.upload',
    ping: 'scan.priority.ping',
  }
  const canContinue = $derived(
    file !== null &&
      (file.status === 'stopped' || file.status === 'interrupted') &&
      (file.next < file.queue.length || (file.mode === 'thorough' && !file.combosAdded)),
  )

  const rowReason = (r: ScanResult | null) => (r?.outcome === 'measured' ? fiveGReason(r.endc, r.endcRestricted) : null)
  const hasSpread = (r: ScanResult) => r.tests > 1 && r.downloadMinMbps !== null
  const measuredToday = (r: ScanResult | null): r is ScanResult => r?.outcome === 'measured'

  const isCustom = (hex: string) => (file?.customSets ?? []).some((c) => sameBands(c, hex))
  const offerCombos = $derived(
    file !== null && file.mode === 'quick' && (file.customSets ?? []).length === 0 && summary.combosTried === 0,
  )
  /** Sets up a short follow-up scan: only combinations of the bands that worked. */
  function tryCombos() {
    customCombos = suggestCombos(workedBands(file, tones, supported), currentHex).slice(0, MAX_CUSTOM_COMBOS)
    touched = new Set([...touched, ...supported])
    unticked = new Set(supported)
    draftCombo = []
    mode = 'quick'
    opened = 'setup'
  }

  const applyBest = (bands: string) =>
    act(async () => {
      if (!file) return
      await api.applyConfig({ ...$state.snapshot(file.start), lteBands: bands })
      bandEdit.dirty = false
      await api.dismissScan()
    })
</script>

{#snippet resultsTable()}
  <div class="scroll">
    <table>
      <thead>
        {#if showAverage}
          <tr class="groups">
            <td colspan="4"></td>
            <th scope="colgroup" colspan="3" class="n">{t('scan.col.today')}</th>
            <th scope="colgroup" colspan="3" class="n avg-start">
              <span class="head">{t('scan.col.average')}<InfoTip label={t('metrics.whatIs', { name: t('scan.col.average') })} text={t('scan.help.average')} /></span>
            </th>
          </tr>
        {/if}
        <tr>
          <th scope="col" class="n">#</th>
          <th scope="col">{t('scan.col.bands')}</th>
          <th scope="col">{t('scan.col.serving')}</th>
          <th scope="col">
            <span class="head">5G<InfoTip label={t('metrics.whatIs', { name: '5G' })} text={t('scan.help.got5g')} /></span>
          </th>
          <th scope="col" class="n">{t('speedtest.phase.download')}</th>
          <th scope="col" class="n">{t('speedtest.phase.upload')}</th>
          {#if !showAverage}
            <th scope="col" class="n">
              <span class="head">{t('speedtest.phase.idlePing')}<InfoTip label={t('metrics.whatIs', { name: t('speedtest.phase.idlePing') })} text={t('metrics.help.idlePing')} /></span>
            </th>
          {/if}
          <th scope="col" class="n">
            <span class="head">{t('speedtest.phase.busy')}<InfoTip label={t('speedtest.busyHelpLabel')} text={t('speedtest.busyHelp')} /></span>
          </th>
          {#if showAverage}
            <th scope="col" class="n avg-start">{t('speedtest.phase.download')}</th>
            <th scope="col" class="n">{t('speedtest.phase.upload')}</th>
            <th scope="col" class="n">{t('speedtest.phase.busy')}</th>
          {/if}
        </tr>
      </thead>
      <tbody>
        {#each tableRows as row, i (row.bands)}
          {@const r = row.result}
          {@const c = row.numbers}
          {@const busyMs = r ? busyPingOf(r) : null}
          {@const lvl = latLevel(busyMs)}
          {@const avgLvl = latLevel(c?.busyPingMs ?? null)}
          {@const why = rowReason(r)}
          {@const got5g = c?.got5g ?? r?.got5g ?? false}
          {@const nrBand = c?.nrBand ?? r?.nrBand ?? null}
          <tr class:best={i === 0 && pick !== null} data-testid="scan-row">
            <td class="n">{c ? formatNumber(i + 1, { digits }) : '—'}</td>
            <td>
              <span class="strong"><BandTags bands={row.bands} allLabel={t('bands.allSupported')} total={allBands} /></span>
              {#if r?.isStart || (file && sameBands(row.bands, file.start.lteBands))}<span class="k">{t('scan.current')}</span>{/if}
              {#if isCustom(row.bands)}<span class="k tag-custom" data-testid="scan-your-combo">{t('scan.yourCombo')}</span>{/if}
            </td>
            <td>
              {#if r && r.lteBand !== null}<bdi dir="ltr">B{r.lteBand}</bdi> <span class="k">PCI {formatNumber(r.ltePci, { digits })}</span>{:else}—{/if}
            </td>
            <td>
              {#if got5g}<span class="yes"><bdi dir="ltr">{nrBand !== null ? `n${nrBand}` : '5G'}</bdi></span>{:else}<span class="k">{t('scan.no5g')}</span>{#if why}<span class="k why" data-testid="no-5g-reason">{t(`scan.noNrReason.${why}`)}</span>{/if}{/if}
            </td>
            {#if measuredToday(r)}
              <td class="n" data-testid="today-down">
                <bdi dir="ltr">{mbps(r.downloadMbps, digits)}</bdi>
                {#if hasSpread(r)}<span class="spread"><bdi dir="ltr">{mbps(r.downloadMinMbps, digits)}–{mbps(r.downloadMaxMbps, digits)}</bdi></span>{/if}
              </td>
              <td class="n">
                <bdi dir="ltr">{mbps(r.uploadMbps, digits)}</bdi>
                {#if hasSpread(r)}<span class="spread"><bdi dir="ltr">{mbps(r.uploadMinMbps, digits)}–{mbps(r.uploadMaxMbps, digits)}</bdi></span>{/if}
              </td>
              {#if !showAverage}<td class="n"><bdi dir="ltr">{ms(r.idlePingMs, digits)}</bdi></td>{/if}
              <td class="n">
                <bdi dir="ltr" class={lvl ? `lat-${lvl}` : ''}>{ms(busyMs, digits)}</bdi>
                {#if lvl && lvl !== 'low'}<span class="spread" data-testid="ping-level">{t(`speedtest.lat.${lvl}`)}</span>{/if}
              </td>
            {:else}
              <td colspan={showAverage ? 3 : 4} class="k wrap" title={row.result?.note ?? undefined}>{row.result ? t(OUTCOME[row.result.outcome]) : t('scan.notReached')}</td>
            {/if}
            {#if showAverage}
              {#if c}
                <td class="n avg-start" data-testid="average-down">
                  <bdi dir="ltr" class="strong">{mbps(c.downloadMbps, digits)}</bdi>
                  <span class="spread">{t('scan.testsCount', { n: num(c.tests) })}</span>
                </td>
                <td class="n"><bdi dir="ltr" class="strong">{mbps(c.uploadMbps, digits)}</bdi></td>
                <td class="n">
                  <bdi dir="ltr" class="strong {avgLvl ? `lat-${avgLvl}` : ''}">{ms(c.busyPingMs, digits)}</bdi>
                  {#if avgLvl && avgLvl !== 'low'}<span class="spread">{t(`speedtest.lat.${avgLvl}`)}</span>{/if}
                </td>
              {:else}
                <td colspan="3" class="n k avg-start">—</td>
              {/if}
            {/if}
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
{/snippet}

{#snippet bandPick(b: number, disabled: boolean)}
  {@const tone = tones.get(b)}
  <label class="pick duplex-{duplexOf(b)}" class:on={isOn(b) && !disabled} class:dead={tone === 'noService'} class:off={disabled}>
    <input type="checkbox" checked={isOn(b) && !disabled} {disabled} onchange={() => toggle(b)} />
    <bdi dir="ltr">B{b}</bdi>{#if tone}<BandToneMark {tone} />{/if}
  </label>
{/snippet}

{#snippet sectionTitle(n: number, key: MessageKey, id?: string)}
  <h3 class="sec-title" {id}><span class="sec-num" aria-hidden="true">{formatNumber(n, { digits })}</span>{t(key)}</h3>
{/snippet}

<nav class="steps" aria-label={t('scan.flow.label')} data-testid="scan-steps">
  <ol>
    {#each STEPS as s, i (s)}
      <li class:done={i < stepAt} class:current={i === stepAt}>
        <button type="button" disabled={!stepOpen(s)} aria-current={i === stepAt ? 'step' : undefined} onclick={() => (opened = s)}>
          <span class="mark" aria-hidden="true">{#if i < stepAt}<Check size={13} />{:else}{formatNumber(i + 1, { digits })}{/if}</span>
          {t(`scan.flow.${s}`)}
        </button>
      </li>
    {/each}
  </ol>
</nav>

{#if step === 'setup'}
  {#if spectrum}
    <section class="order" aria-labelledby="scan-order" data-testid="scan-order">
      <div class="order-head">
        <h3 id="scan-order" class="order-title">{t('spectrum.scanOrder')}</h3>
        <span class="k">{t('scan.about', { value: t('scan.minutes', { n: num(estimate.minutes) }) })}</span>
      </div>
      <ol class="order-strip">
        <li class="first">{t('scan.current')}</li>
        {#each chosen as b (b)}<li><bdi dir="ltr">B{b}</bdi></li>{/each}
        {#each customCombos as c, i (i)}<li><bdi dir="ltr">{c.map((b) => `B${b}`).join(' + ')}</bdi></li>{/each}
      </ol>
    </section>
  {/if}
  <div class="setup">
    <section class="card options" aria-label={t('scan.title')}>
      <fieldset class="line">
        <legend>{@render sectionTitle(1, 'scan.mode')}</legend>
        <div class="seg">
          {#each ['quick', 'thorough'] as const as m (m)}
            <label class:on={mode === m}>
              <input class="sr-only" type="radio" name="scan-mode" value={m} bind:group={mode} />
              {t(`scan.modes.${m}`)}
            </label>
          {/each}
        </div>
        <p class="k">{t(`scan.modes.${mode}Hint`)}</p>
      </fieldset>

      <fieldset class="line" data-testid="scan-singles">
        <legend>{@render sectionTitle(2, 'scan.section.singles')}</legend>
        <p class="k">{t('scan.bandsHint')}</p>
        {#each GROUPS as d (d)}
          {@const list = groups[d]}
          {#if list.length > 0}
            {@const off = pickDisabled(d)}
            <div class="sub" data-testid={d === 'sdl' ? 'scan-add-ons' : `scan-group-${d}`}>
              <div class="sub-head">
                <DuplexHeading duplex={d} level="h4" count={off ? 0 : list.filter(isOn).length} total={list.length} />
                <span class="all-none">
                  <button
                    type="button"
                    class="text-btn"
                    disabled={off || blocked !== null}
                    aria-label={t('scan.selectGroup', { group: KIND[d] })}
                    onclick={() => setGroup(list, true)}>{t('scan.selectAll')}</button
                  >
                  <button
                    type="button"
                    class="text-btn"
                    disabled={off || blocked !== null}
                    aria-label={t('scan.clearGroup', { group: KIND[d] })}
                    onclick={() => setGroup(list, false)}>{t('scan.selectNone')}</button
                  >
                </span>
              </div>
              <div class="picks">
                {#each list as b (b)}{@render bandPick(b, off)}{/each}
              </div>
              {#if d === 'sdl'}<p class="k">{t('scan.addOnsHint')}</p>{/if}
            </div>
          {/if}
        {/each}
      </fieldset>

      <section class="line" aria-labelledby="scan-combos-title" data-testid="scan-combos">
        {@render sectionTitle(3, 'scan.section.combos', 'scan-combos-title')}
        <p class="k">{t(mode === 'thorough' ? 'scan.combos.leadThorough' : 'scan.combos.leadQuick')}</p>

        <div class="sub">
          <h4 class="sub-title">{t('scan.combos.suggested')}</h4>
          {#if worked.length < 2}
            <p class="k" data-testid="scan-combos-none">{t('scan.combos.noneYet')}</p>
          {:else if suggestions.length === 0}
            <p class="k" data-testid="scan-combos-none">{t('scan.combos.onlyCurrent')}</p>
          {:else}
            <div class="picks" data-testid="scan-suggestions">
              {#each suggestions as combo (combo.join('+'))}
                {@const added = isAdded(combo)}
                <button
                  type="button"
                  class="chip"
                  class:on={added}
                  aria-pressed={added}
                  data-testid="scan-suggestion"
                  disabled={!added && customCombos.length >= MAX_CUSTOM_COMBOS}
                  onclick={() => toggleCombo(combo)}
                >
                  {#if added}<Check size={14} aria-hidden="true" />{:else}<Plus size={14} aria-hidden="true" />{/if}
                  <BandTags bands={bandsToHex(combo)} />
                </button>
              {/each}
            </div>
          {/if}
        </div>

        <div class="sub" data-testid="scan-builder">
          <h4 class="sub-title">{t('scan.combos.build')}</h4>
          <div class="picks builder">
            {#each GROUPS as d (d)}
              {#if groups[d].length > 0}
                <span class="mini-lbl duplex-{d}" aria-hidden="true">{KIND[d]}</span>
                {#each groups[d] as b (b)}
                  <button
                    type="button"
                    class="chip mini duplex-{d}"
                    class:on={draftCombo.includes(b)}
                    aria-pressed={draftCombo.includes(b)}
                    title={KIND[d]}
                    data-testid="scan-build-band"
                    onclick={() => toggleDraft(b)}><bdi dir="ltr">B{b}</bdi></button
                  >
                {/each}
              {/if}
            {/each}
          </div>
          <div class="build-row">
            {#if draftCombo.length > 0}<span class="draft" data-testid="scan-draft"><BandTags bands={bandsToHex(draftCombo)} /></span>{/if}
            <button type="button" data-testid="scan-add-combo" disabled={!canAddCombo} onclick={addDraft}>
              <Plus size={16} aria-hidden="true" />{t('scan.combos.add')}
            </button>
          </div>
          {#if comboProblem && draftCombo.length > 0}<p class="k" data-testid="scan-combo-problem">{t(comboProblem)}</p>{/if}
        </div>

        {#if customCombos.length > 0}
          <div class="sub">
            <h4 class="sub-title">{t('scan.combos.added')}</h4>
            <ul class="custom-list" data-testid="scan-custom-list">
              {#each comboMasks as hex, i (hex)}
                <li>
                  <BandTags bands={hex} />
                  <button
                    type="button"
                    class="icon-btn"
                    aria-label={t('scan.combos.remove', { bands: bandsLabel(hex) })}
                    title={t('scan.combos.remove', { bands: bandsLabel(hex) })}
                    onclick={() => removeCombo(i)}><X size={14} aria-hidden="true" /></button
                  >
                </li>
              {/each}
            </ul>
          </div>
        {/if}
      </section>

      <fieldset class="line">
        <legend>{@render sectionTitle(4, 'scan.repeats')}</legend>
        <div class="seg">
          {#each SCAN_REPEATS as n (n)}
            <label class:on={repeats === n}>
              <input class="sr-only" type="radio" name="scan-repeats" value={n} checked={repeats === n} onchange={() => (pickedRepeats = n)} />
              {formatNumber(n, { digits })}
            </label>
          {/each}
        </div>
        <p class="k">{t('scan.repeatsHint')}</p>
      </fieldset>
    </section>

    <aside class="card plan" aria-label={t('scan.start')}>
      <p class="k" data-testid="scan-breakdown">
        {t('scan.breakdown', { singles: num(chosen.length), combos: num(customCombos.length) })}{#if mode === 'thorough'}{t('scan.breakdownAuto')}{/if}
      </p>
      <dl class="estimate stacked" data-testid="scan-estimate">
        <div><dt>{t('scan.configs')}</dt><dd>{mode === 'thorough' ? t('scan.upTo', { n: num(estimate.configs) }) : num(estimate.configs)}</dd></div>
        <div><dt>{t('scan.time')}</dt><dd>{t('scan.about', { value: t('scan.minutes', { n: num(estimate.minutes) }) })}</dd></div>
        <div><dt>{t('scan.data')}</dt><dd>{t('scan.about', { value: dataText })}</dd></div>
      </dl>

      <div class="warn" role="note">
        <TriangleAlert size={16} aria-hidden="true" />
        <div class="warn-text">
          <p class="warn-lead">{warningLead}</p>
          {#if warningRest}<p>{warningRest}</p>{/if}
          <p>{t('scan.warningBrowser')}</p>
        </div>
      </div>

      <div class="actions column">
        <button
          type="button"
          class="primary"
          disabled={busy || (chosen.length === 0 && customCombos.length === 0) || blocked !== null}
          onclick={start}
        >
          <Play size={16} aria-hidden="true" />{t('scan.start')}
        </button>
        {#if file}<button type="button" data-testid="scan-back-results" onclick={() => (opened = 'results')}>{t('scan.flow.backResults')}</button>{/if}
        {#if blocked}<p class="k" data-testid="scan-blocked">{t(blocked)}</p>{/if}
      </div>
    </aside>
  </div>
{:else if step === 'scanning' && file}
  <section class="card" aria-labelledby="scan-running" data-testid="scan-running">
    <div class="top">
      <h2 id="scan-running">{t(running ? 'scan.running' : 'scan.flow.tried')}</h2>
      {#if running}
        <button type="button" class="stop" disabled={busy} onclick={() => act(api.stopScan)}>
          <CircleStop size={16} aria-hidden="true" />{t(file.status === 'returning' ? 'scan.stopReturn' : 'scan.stop')}
        </button>
      {/if}
    </div>

    {#if running}
      <div class="now" aria-live="polite">
        {#if file.status === 'running' && currentBands}
          <span class="k">{t('scan.position', { n: num(position), total: num(file.queue.length) })}</span>
          <span class="now-bands"><BandTags bands={currentBands} allLabel={t('bands.allSupported')} total={allBands} /></span>
        {/if}
        <span class="k">
          {t(stepKey)}
          {#if file.status === 'running' && file.step === 'testing' && file.repeats > 1}
            {t('scan.testOf', { n: num(file.test), total: num(file.repeats) })}
          {/if}
        </span>
      </div>

      <div
        class="bar"
        role="progressbar"
        aria-label={t('scan.running')}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={Math.round(fraction * 100)}
      >
        <i style:inline-size="{fraction * 100}%"></i>
      </div>

      {#if file.status === 'running' && file.step === 'testing'}
        <dl class="estimate">
          {#each STAGES as [id, label] (id)}
            <div>
              <dt>{t(label)}</dt>
              <dd class="num">
                <bdi dir="ltr">{id === 'idlePing' ? ms(speed.live.idlePingMs, digits) : mbps(id === 'download' ? speed.live.downloadMbps : speed.live.uploadMbps, digits)}</bdi>
                <span class="k">{t(id === 'idlePing' ? 'speedtest.unit.ms' : 'speedtest.unit.mbps')}</span>
              </dd>
            </div>
          {/each}
        </dl>
      {/if}
      <p class="k">{t(file.status === 'returning' ? 'scan.stopReturnHint' : 'scan.stopHint')}</p>
    {/if}

    {#if file.results.length > 0}
      <ol class="timeline" data-testid="scan-timeline">
        {#each file.results as r (r.bands)}
          <li>
            <span class="event {r.outcome}" aria-hidden="true">
              {#if r.outcome === 'measured'}<Check size={13} />{:else if r.outcome === 'noService'}<X size={13} />{:else}<TriangleAlert size={13} />{/if}
            </span>
            <span class="what">
              <span class="strong"><BandTags bands={r.bands} allLabel={t('bands.allSupported')} total={allBands} /></span>
              {#if r.isStart}<span class="k">{t('scan.current')}</span>{/if}
            </span>
            {#if r.outcome === 'measured'}
              <span class="result figs" title={figures(r)}>
                <span class="fig"><ArrowDown size={13} aria-hidden="true" /><bdi dir="ltr">{mbps(r.downloadMbps, digits)}</bdi> <span class="k">{t('speedtest.unit.mbps')}</span></span>
                <span class="fig"><ArrowUp size={13} aria-hidden="true" /><bdi dir="ltr">{mbps(r.uploadMbps, digits)}</bdi> <span class="k">{t('speedtest.unit.mbps')}</span></span>
                <span class="fig-5g">{#if r.got5g}<span class="yes"><bdi dir="ltr">{r.nrBand !== null ? `n${r.nrBand}` : '5G'}</bdi></span>{:else}<span class="k">{t('scan.no5g')}</span>{/if}</span>
              </span>
            {:else}
              <span class="result">{t(OUTCOME[r.outcome])}</span>
            {/if}
            <span class="k at">{whenLabel(r.atMs, Date.now(), digits)}</span>
          </li>
        {/each}
      </ol>
    {/if}
    {#if running && file.status === 'running' && upNext.length > 0}
      <p class="k" data-testid="scan-up-next">{t('scan.timeline.next', { list: upNext.map((b) => isolate(bandsLabel(b))).join(t('bands.listSep')) })}</p>
    {/if}
    {#if !running}
      <div class="actions">
        <button type="button" data-testid="scan-see-results" onclick={() => (opened = 'results')}>{t('scan.flow.seeResults')}</button>
      </div>
    {/if}
  </section>
{:else if file}
  <div class="results" data-testid="scan-results">
    <section class="card" aria-labelledby="scan-summary-title" data-testid="scan-summary">
      <h2 id="scan-summary-title" class="sr-only">{t('scan.results')}</h2>
      {#if file.returnFailed}
        <p class="warn" role="alert"><TriangleAlert size={16} aria-hidden="true" /><span>{t('scan.returnFailed')}</span></p>
      {:else if file.status === 'interrupted'}
        <p class="warn" role="note"><TriangleAlert size={16} aria-hidden="true" /><span>{t('scan.interrupted')}</span></p>
      {:else if file.status === 'stopped'}
        <p class="k">{t('scan.stopped')}</p>
      {/if}

      {#if pick}
        <div class="answer">
          <p class="headline">{t('scan.summary.best', { bands: namedBands(pick.bands) })}</p>
          <p class="lead-line">
            {#if pickIsStart}{t('scan.summary.keep')}{:else if gainPercent !== null}{t('scan.summary.faster', { pct: num(gainPercent) })}{:else}{t('scan.summary.switch', { bands: namedBands(pick.bands) })} {why}{/if}
          </p>
          <p class="k" data-testid="scan-time-of-day">
            {#if swingsWidely(pick)}{t('scan.summary.swing', { low: isolate(mbps(pick.lowMbps, digits)), high: isolate(mbps(pick.highMbps, digits)) })} {/if}{t('scan.summary.timeOfDay')}
          </p>
        </div>
        {#if pick.earlier > 0}
          <p class="k" data-testid="scan-average-note">{t('scan.summary.averageOf', { n: num(pick.tests) })}</p>
        {/if}
        <dl class="estimate pick-figures" data-testid="scan-pick">
          <div>
            <dt>{t('speedtest.phase.download')}</dt>
            <dd><bdi dir="ltr">{mbps(pick.downloadMbps, digits)}</bdi> <span class="k">{t('speedtest.unit.mbps')}</span></dd>
            {#if pick.earlier > 0 && measuredToday(pickToday)}<dd class="today">{t('scan.summary.today', { value: isolate(mbps(pickToday.downloadMbps, digits)) })}</dd>{/if}
          </div>
          <div>
            <dt>{t('speedtest.phase.upload')}</dt>
            <dd><bdi dir="ltr">{mbps(pick.uploadMbps, digits)}</bdi> <span class="k">{t('speedtest.unit.mbps')}</span></dd>
            {#if pick.earlier > 0 && measuredToday(pickToday)}<dd class="today">{t('scan.summary.today', { value: isolate(mbps(pickToday.uploadMbps, digits)) })}</dd>{/if}
          </div>
          <div>
            <dt>{t('speedtest.phase.busy')}</dt>
            <dd><bdi dir="ltr">{ms(pick.busyPingMs, digits)}</bdi> <span class="k">{t('speedtest.unit.ms')}</span></dd>
            {#if pick.earlier > 0 && measuredToday(pickToday)}<dd class="today">{t('scan.summary.today', { value: isolate(ms(busyPingOf(pickToday), digits)) })}</dd>{/if}
          </div>
          <div><dt>5G</dt><dd>{#if pick.got5g}<span class="yes"><bdi dir="ltr">{pick.nrBand !== null ? `n${pick.nrBand}` : '5G'}</bdi></span>{:else}{t('scan.no5g')}{/if}</dd></div>
        </dl>
        {#if !pick.measuredNow}
          <p>{t('scan.summary.onlyEarlier', { n: num(pick.earlier) })}</p>
        {/if}
      {:else if file.results.length > 0}
        <p class="headline">{t('scan.summary.notEnough')}</p>
      {:else}
        <p>{t('scan.noResults')}</p>
      {/if}

      <div class="actions">
        {#if pick && !pickIsStart}
          <button type="button" class="primary" disabled={busy} onclick={() => applyBest(pick.bands)}>
            {t('scan.applyBest', { bands: isolate(bandsLabel(pick.bands)) })}
          </button>
        {/if}
        <button type="button" disabled={busy} data-testid="scan-keep" onclick={() => act(api.dismissScan)}>{t('scan.keepCurrent')}</button>
        {#if canContinue}
          <button type="button" disabled={busy} data-testid="scan-resume" onclick={() => act(api.continueScan)}>{t('scan.continue')}</button>
          <button type="button" class="quiet" disabled={busy} data-testid="scan-restart" onclick={restart}>{t('scan.restart')}</button>
        {:else}
          <button type="button" class="quiet" disabled={busy} data-testid="scan-run-again" onclick={restart}>{t('scan.runAgain')}</button>
        {/if}
        <button type="button" class="quiet" disabled={busy} data-testid="scan-change-setup" onclick={() => (opened = 'setup')}>{t('scan.flow.backSetup')}</button>
      </div>

      <div class="notes">
        {#if includesAuto}<p data-testid="scan-includes-auto">{t('scan.summary.includesAuto')}</p>{/if}
        {#if summary.lose5g.length > 0}
          <p>{t('scan.summary.lose5g', { list: nameList(summary.lose5g) })}</p>
        {/if}
        {#if summary.inconclusive.length > 0}
          <p>{t('scan.summary.inconclusive', { list: nameList(summary.inconclusive) })}</p>
        {/if}
        <p>
          {summary.bandsTried >= triable
            ? t('scan.summary.triedAll', { n: num(triable) })
            : t('scan.summary.triedSome', { n: num(summary.bandsTried), total: num(triable) })}
          <!-- Only urges combinations when there is one to offer (not just the current set). -->
          {#if !offerCombos}{t('scan.summary.combos', { n: num(summary.combosTried) })}{:else if suggestions.length > 0}{t(
              'scan.summary.noCombos2',
            )}{/if}
        </p>
        {#if offerCombos && suggestions.length > 0}
          <div class="actions">
            <button type="button" disabled={busy} data-testid="scan-try-combos" onclick={tryCombos}>{t('scan.combos.tryWorked')}</button>
          </div>
        {/if}
        {#if pick?.measuredNow}
          <p>
            {summary.tests > 1 ? t('scan.summary.testedMany', { n: num(summary.tests) }) : t('scan.summary.testedOnce')}
          </p>
        {/if}
        <p><button type="button" class="link" onclick={seeBandSets}>{t('history.link.scan')}</button></p>
      </div>
    </section>

    {#if tableRows.length > 0}
      <section class="card" aria-labelledby="scan-all">
        <div class="top">
          <h2 id="scan-all">{t('scan.ranked')}</h2>
          <fieldset class="rank-by">
            <legend class="sr-only">{t('scan.priority.label')}</legend>
            <span class="k" aria-hidden="true">{t('scan.priority.label')}</span>
            <div class="seg">
              {#each SCAN_PRIORITIES as p (p)}
                <label class:on={priority === p}>
                  <input class="sr-only" type="radio" name="scan-priority" value={p} bind:group={priority} />
                  {t(PRIORITY[p])}
                </label>
              {/each}
            </div>
          </fieldset>
        </div>
        {@render resultsTable()}
      </section>
    {/if}

    {#if summary.noService.length > 0}
      <details class="card dead-list" data-testid="scan-no-service">
        <summary>
          <ChevronDown size={16} aria-hidden="true" />
          {t('scan.noServiceList', { n: num(summary.noService.length) })}
        </summary>
        <ul>
          {#each summary.noService as r (r.bands)}<li><BandTags bands={r.bands} allLabel={t('bands.allSupported')} total={allBands} /></li>{/each}
        </ul>
        <p class="k">{t('scan.noServiceHint')}</p>
      </details>
    {/if}
  </div>
{/if}

<style>
  .card {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius-card);
    padding-block: var(--card-pad-block);
    padding-inline: var(--card-pad-inline);
    display: flex;
    flex-direction: column;
    gap: 12px;
    min-inline-size: 0;
  }
  p {
    margin: 0;
  }
  .k {
    color: var(--muted);
    font-size: var(--fs-small);
  }
  .answer {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .headline {
    font-family: var(--font-heading);
    font-size: var(--fs-title);
    font-weight: 700;
    line-height: 1.25;
  }
  .lead-line {
    font-size: var(--fs-lead);
    font-weight: 500;
  }
  .pick-figures dd {
    font-size: var(--fs-title);
    line-height: 1.25;
  }
  .pick-figures dd.today {
    font-size: var(--fs-small);
  }
  .figs {
    display: grid;
    grid-template-columns: minmax(96px, auto) minmax(96px, auto) minmax(60px, 1fr);
    gap: 4px 16px;
    align-items: baseline;
  }
  .fig {
    display: inline-flex;
    align-items: baseline;
    gap: 4px;
    font-variant-numeric: tabular-nums;
  }
  .fig :global(svg) {
    align-self: center;
    color: var(--muted);
  }
  .why {
    display: block;
  }
  .spread {
    display: block;
    color: var(--muted);
    font-size: var(--fs-small);
    font-weight: 400;
  }
  .strong {
    font-weight: 600;
  }
  .top {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }
  h2 {
    margin: 0;
    font-size: var(--fs-section);
    font-weight: 600;
  }
  /* Choices on the wide side, then what they cost and the button that starts it. */
  .setup {
    display: grid;
    grid-template-columns: minmax(0, 1fr) clamp(264px, 32%, 400px);
    align-items: start;
    gap: var(--gap);
  }
  @media (max-width: 820px) {
    .setup {
      grid-template-columns: minmax(0, 1fr);
    }
  }
  .options {
    gap: 20px;
  }
  /* Spectrum fills the width: each step is its own block, the two short ones (how much, tests per
     configuration) beside each other above the two long ones (single bands, combinations). */
  @media (min-width: 1100px) {
    :global(:root[data-design='spectrum']) .options {
      display: grid;
      grid-template-columns: repeat(2, minmax(0, 1fr));
      align-items: stretch;
      gap: var(--gap);
      padding: 0;
      background: none;
      box-shadow: none;
    }
    :global(:root[data-design='spectrum']) .options > .line {
      box-sizing: border-box;
      min-inline-size: 0;
      margin: 0;
      border: 0;
      border-radius: var(--radius-card);
      background: var(--surface);
      padding-block: var(--card-pad-block);
      padding-inline: var(--card-pad-inline);
    }
    :global(:root[data-design='spectrum']) .options > .line:nth-child(1) {
      order: 1;
    }
    :global(:root[data-design='spectrum']) .options > .line:nth-child(4) {
      order: 2;
    }
    :global(:root[data-design='spectrum']) .options > .line:nth-child(2) {
      order: 3;
    }
    :global(:root[data-design='spectrum']) .options > .line:nth-child(3) {
      order: 4;
    }
    /* The band boxes share the row evenly instead of bunching at one end. */
    /* A legend normally sits on the block's top edge; floated, it is laid out inside it. */
    :global(:root[data-design='spectrum']) .options > .line > legend {
      float: inline-start;
      inline-size: 100%;
    }
    :global(:root[data-design='spectrum']) .options > .line:nth-child(2) .picks {
      display: grid;
      grid-template-columns: repeat(auto-fill, minmax(86px, 1fr));
    }
    :global(:root[data-design='spectrum']) .setup {
      grid-template-columns: minmax(0, 1fr) clamp(264px, 24%, 360px);
    }
  }
  .plan {
    gap: 14px;
    position: sticky;
    inset-block-start: 8px;
  }
  @media (max-width: 820px) {
    .plan {
      position: static;
    }
  }
  .results {
    display: flex;
    flex-direction: column;
    gap: var(--gap);
  }
  .now {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .now-bands {
    font-size: var(--fs-title);
    font-weight: 600;
    line-height: 1.25;
  }
  .steps ol {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 4px 8px;
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .steps li {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  /* The line between two steps. */
  .steps li + li::before {
    content: '';
    inline-size: 32px;
    block-size: 1px;
    background: var(--border);
  }
  .steps button {
    all: unset;
    display: inline-flex;
    align-items: center;
    gap: 8px;
    min-block-size: var(--control-height);
    padding-inline: 6px;
    border-radius: var(--radius-control);
    color: var(--muted);
    cursor: pointer;
  }
  .steps button:disabled {
    color: var(--faint);
    cursor: default;
  }
  .steps button:hover:not(:disabled) {
    color: var(--text);
  }
  .steps button:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
  .steps .mark {
    display: grid;
    place-items: center;
    inline-size: 22px;
    block-size: 22px;
    border: 1.5px solid currentColor;
    border-radius: 50%;
    font-size: 12px;
    font-variant-numeric: tabular-nums;
  }
  .steps .done .mark {
    border-color: var(--accent);
    color: var(--accent);
  }
  .steps .current button {
    color: var(--text);
    font-weight: 600;
  }
  .steps .current .mark {
    border-color: var(--accent);
    background: var(--accent);
    color: var(--ground);
  }
  .timeline {
    display: flex;
    flex-direction: column;
    margin: 0;
    padding: 0;
    list-style: none;
    border-block-start: 1px solid var(--rule);
  }
  .timeline li {
    display: grid;
    grid-template-columns: 20px minmax(110px, 180px) minmax(0, 1fr) auto;
    align-items: center;
    gap: 2px 10px;
    padding-block: 8px;
    border-block-end: 1px solid var(--rule);
  }
  .event {
    display: grid;
    place-items: center;
    inline-size: 20px;
    block-size: 20px;
    border-radius: 50%;
    background: var(--active);
    color: var(--muted);
  }
  .event.measured {
    color: var(--q-excellent);
  }
  .event.noService {
    color: var(--q-poor);
  }
  .event.testFailed,
  .event.failed {
    color: var(--q-fair);
  }
  .at {
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
  }
  @media (max-width: 760px) {
    .timeline li {
      grid-template-columns: 20px minmax(0, 1fr);
    }
    .result,
    .at {
      grid-column: 2;
    }
  }
  button.link {
    all: unset;
    color: var(--accent);
    text-decoration: underline;
    text-underline-offset: 2px;
    cursor: pointer;
  }
  button.link:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  .notes {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding-block-start: 12px;
    border-block-start: 1px solid var(--rule);
    color: var(--muted);
    font-size: var(--fs-small);
    max-inline-size: var(--measure);
  }
  .rank-by {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 4px 8px;
  }
  .dead-list {
    display: block;
  }
  .dead-list summary {
    display: flex;
    align-items: center;
    gap: 8px;
    font-weight: 500;
    cursor: pointer;
    list-style: none;
  }
  .dead-list summary::-webkit-details-marker {
    display: none;
  }
  .dead-list summary :global(svg) {
    color: var(--muted);
    flex: none;
    transition: rotate 0.15s ease;
  }
  .dead-list[open] summary :global(svg) {
    rotate: 180deg;
  }
  .dead-list summary:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  .dead-list ul {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-block: 12px 8px;
    margin-inline: 0;
    padding: 0;
    list-style: none;
  }
  .dead-list li {
    padding-block: 3px;
    padding-inline: 10px;
    border: 1px dashed var(--border);
    border-radius: var(--radius-control);
    font-size: var(--fs-small);
    font-variant-numeric: tabular-nums;
  }
  @media (prefers-reduced-motion: reduce) {
    .dead-list summary :global(svg) {
      transition: none;
    }
  }
  fieldset {
    border: 0;
    margin: 0;
    padding: 0;
    min-inline-size: 0;
  }
  .line {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  legend {
    padding: 0;
    margin-block-end: 6px;
  }
  /* Numbered sections of the setup, each a heading and a lead line. */
  .sec-title {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 0;
    font-size: var(--fs-section);
    font-weight: 600;
  }
  .sec-num {
    display: grid;
    place-items: center;
    flex: none;
    inline-size: 22px;
    block-size: 22px;
    border-radius: 50%;
    background: var(--secondary);
    color: var(--muted);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
  }
  .sub {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding-block-start: 8px;
  }
  .sub-head {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 4px 12px;
  }
  .sub-title {
    margin: 0;
    color: var(--muted);
    font-size: 13px;
    font-weight: 500;
  }
  .all-none {
    display: inline-flex;
    gap: 2px;
  }
  .text-btn {
    min-block-size: 26px;
    padding-inline: 6px;
    border-color: transparent;
    background: transparent;
    color: var(--accent);
    font-size: var(--fs-small);
  }
  .text-btn:disabled {
    color: var(--faint);
  }
  /* A chip's band kind as a stripe on its start edge, apart from the border so the selection never
     covers it. The headings and titles say the kind in words. */
  .pick,
  .chip.mini {
    position: relative;
  }
  .pick::before,
  .chip.mini::before {
    content: '';
    position: absolute;
    inset-block: -1px;
    inset-inline-start: -1px;
    inline-size: 3px;
    border-start-start-radius: var(--radius-control);
    border-end-start-radius: var(--radius-control);
    background: var(--duplex, var(--border));
  }
  .pick :global(.tone-mark) {
    margin-inline-start: -2px;
  }
  .chip {
    gap: 6px;
    min-block-size: 32px;
    padding-inline: 8px 10px;
    border: 1px solid var(--border);
    border-radius: var(--radius-control);
    background: var(--surface);
    color: var(--muted);
    font-weight: 400;
  }
  .chip:hover:not(:disabled) {
    border-color: var(--field-border);
    color: var(--text);
  }
  .chip.on {
    border-color: var(--field-border);
    background: var(--raised);
    color: var(--text);
    font-weight: 500;
  }
  .chip :global(svg) {
    flex: none;
  }
  .chip.mini {
    min-block-size: 28px;
    padding-inline: 9px 8px;
    font-size: var(--fs-small);
  }
  .builder {
    align-items: center;
  }
  .mini-lbl {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    margin-inline-start: 4px;
    color: var(--muted);
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.04em;
  }
  .mini-lbl:first-child {
    margin-inline-start: 0;
  }
  .mini-lbl::before {
    content: '';
    inline-size: 8px;
    block-size: 8px;
    border-radius: 2px;
    background: var(--duplex);
  }
  .build-row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px 12px;
  }
  .custom-list {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .custom-list li {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding-block: 2px;
    padding-inline: 8px 2px;
    border: 1px solid var(--field-border);
    border-radius: var(--radius-control);
    background: var(--raised);
  }
  .icon-btn {
    min-block-size: 26px;
    padding-inline: 5px;
    border-color: transparent;
    background: transparent;
    color: var(--muted);
  }
  .icon-btn:hover {
    color: var(--text);
  }
  .tag-custom {
    margin-inline-start: 6px;
  }
  .seg {
    display: inline-flex;
    flex-wrap: wrap;
    align-self: flex-start;
    gap: 2px;
    padding: 2px;
    border-radius: var(--radius-control);
    background: var(--secondary);
  }
  .seg label {
    display: inline-flex;
    align-items: center;
    block-size: 30px;
    padding-inline: 12px;
    border-radius: 5px;
    font-size: 14px;
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
  .seg label:has(input:focus-visible) {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
  .picks {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .pick {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    block-size: 32px;
    padding-inline: 8px 10px;
    border: 1px solid var(--border);
    border-radius: var(--radius-control);
    color: var(--muted);
    cursor: pointer;
    transition:
      background-color var(--t-fast) var(--ease),
      border-color var(--t-fast) var(--ease),
      color var(--t-fast) var(--ease);
  }
  .pick:hover:not(.off) {
    border-color: var(--field-border);
    color: var(--text);
  }
  .pick.on {
    border-color: var(--field-border);
    background: var(--raised);
    color: var(--text);
    font-weight: 500;
  }
  .pick.dead {
    border-style: dashed;
  }
  .pick.off {
    opacity: 0.6;
    cursor: default;
  }
  .pick input {
    margin: 0;
    accent-color: var(--accent);
  }
  .estimate {
    display: flex;
    flex-wrap: wrap;
    gap: 8px 32px;
    margin: 0;
  }
  /* The bill: one line each, label at the start and figure at the end. */
  .estimate.stacked {
    flex-direction: column;
    gap: 0;
  }
  .estimate.stacked > div {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 12px;
    padding-block: 8px;
    border-block-end: 1px solid var(--rule);
  }
  .estimate.stacked > div:first-child {
    padding-block-start: 0;
  }
  .estimate.stacked dd {
    font-weight: 600;
    text-align: end;
  }
  dt {
    color: var(--muted);
    font-size: var(--fs-small);
  }
  dd {
    margin: 0;
    font-size: var(--fs-value);
    font-variant-numeric: tabular-nums;
  }
  .warn {
    display: flex;
    gap: 8px;
    align-items: flex-start;
    padding-block: 10px;
    padding-inline: 12px;
    border-radius: var(--radius-control);
    background: var(--pill-warning-bg);
    color: var(--pill-warning-ink);
    font-size: var(--fs-small);
    line-height: 1.5;
  }
  .warn :global(svg) {
    flex: none;
    margin-block-start: 2px;
  }
  .warn-text {
    display: flex;
    flex-direction: column;
    gap: 6px;
    color: var(--text);
  }
  .warn-text p:not(.warn-lead) {
    color: var(--muted);
  }
  .warn-lead {
    font-weight: 600;
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  /* The summary has one main action; the rest are there without competing with it. */
  .actions button.quiet {
    background: transparent;
    border-color: transparent;
    color: var(--muted);
    text-decoration: underline;
    text-underline-offset: 3px;
  }
  .actions button.quiet:hover:not(:disabled) {
    color: var(--text);
    background: var(--raised);
  }
  .actions.column {
    flex-direction: column;
  }
  .actions.column button {
    justify-content: center;
  }
  button {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    min-block-size: var(--control-height);
    padding-inline: 14px;
    font-weight: 500;
  }
  .stop {
    border-color: var(--q-poor);
    color: var(--q-poor);
  }
  .bar {
    block-size: 6px;
    border-radius: 3px;
    background: var(--q-track);
    overflow: hidden;
  }
  .bar i {
    display: block;
    block-size: 6px;
    background: var(--accent);
    transition: inline-size 0.3s ease;
  }
  @media (prefers-reduced-motion: reduce) {
    .bar i {
      transition: none;
    }
  }
  .scroll {
    overflow-x: auto;
  }
  table {
    border-collapse: collapse;
    inline-size: 100%;
  }
  th {
    font-weight: 400;
    font-size: var(--fs-small);
    color: var(--muted);
    text-align: start;
    white-space: nowrap;
    padding-block: 0 8px;
    padding-inline: 0 16px;
  }
  .groups th {
    padding-block-end: 2px;
    color: var(--text);
    font-weight: 500;
  }
  .groups td {
    border: 0;
    padding: 0;
  }
  .avg-start {
    border-inline-start: 1px solid var(--rule);
    padding-inline-start: 16px;
  }
  td.wrap {
    white-space: normal;
  }
  dd.today {
    color: var(--muted);
    font-size: var(--fs-small);
  }
  td {
    padding-block: 9px;
    padding-inline: 0 16px;
    border-block-start: 1px solid var(--rule);
    white-space: nowrap;
  }
  .n {
    text-align: end;
    font-variant-numeric: tabular-nums;
  }
  .head {
    display: inline-flex;
    align-items: center;
    gap: 2px;
  }
  tr.best td {
    background: color-mix(in srgb, var(--q-excellent) 9%, var(--surface));
    font-weight: 500;
    border-block-start-color: transparent;
  }
  tr.best td:first-child {
    border-start-start-radius: var(--radius-control);
    border-end-start-radius: var(--radius-control);
  }
  tr.best td:last-child {
    border-start-end-radius: var(--radius-control);
    border-end-end-radius: var(--radius-control);
  }
  tbody tr {
    transition: background-color var(--t-fast) var(--ease);
  }
  tbody tr:not(.best):hover td {
    background: var(--raised);
  }
  .yes {
    color: var(--q-excellent);
    font-weight: 600;
  }
  .lat-high {
    color: var(--q-poor);
  }
  .lat-moderate {
    color: var(--q-fair);
  }

  .order {
    display: flex;
    flex-direction: column;
    gap: 12px;
    background: var(--surface);
    border-radius: var(--radius-card);
    padding-block: var(--card-pad-block);
    padding-inline: var(--card-pad-inline);
  }
  .order-head {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    justify-content: space-between;
    gap: 4px 16px;
  }
  .order-title {
    margin: 0;
    font-size: var(--fs-body);
    font-weight: 600;
  }
  .order-strip {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .order-strip li {
    flex: 1 1 56px;
    min-block-size: 40px;
    display: grid;
    place-items: center;
    padding-inline: 8px;
    border-radius: 8px;
    background: var(--secondary);
    font-weight: 600;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }
  .order-strip li.first {
    flex-grow: 2;
    background: var(--primary);
    color: var(--primary-ink);
  }
</style>
