<script lang="ts">
  // The Auto-scan page: what it needs from the rest of the app, around the scan itself.
  import { onMount } from 'svelte'
  import { api } from '../lib/backend'
  import { app } from '../lib/store.svelte'
  import { t, type MessageKey } from '../lib/i18n'
  import { changeClosed, supportedBands } from '../lib/bands'
  import { bandTones } from '../lib/bandHistory'
  import { ensureSpeedTest, speed } from '../lib/speedtest.svelte'
  import { ensureScan, scan } from '../lib/scan.svelte'
  import type { BandOutcome } from '../lib/bindings/BandOutcome'
  import LoginNeeded from './ui/LoginNeeded.svelte'
  import { needsLogin } from '../lib/login'
  import AutoScan from './AutoScan.svelte'

  const snap = $derived(app.snapshot)
  const closed = $derived(changeClosed(snap?.safeApply.phase) && !snap?.safeApply.sequence)
  const supported = $derived(snap?.capabilities ? supportedBands(snap.capabilities.supportedLte) : [])

  // A history that can't be read just means no band starts unticked.
  let outcomes: BandOutcome[] = $state([])
  const loadOutcomes = () =>
    api
      .bandOutcomes()
      .then((o) => (outcomes = o))
      .catch(() => {})
  onMount(() => {
    void ensureSpeedTest()
    void ensureScan()
  })
  $effect(() => {
    if (closed) void loadOutcomes()
  })
  const tones = $derived(bandTones(speed.history, outcomes, scan.file?.results ?? []))

  const blocked = $derived.by((): MessageKey | null => {
    if (!snap?.capabilities || !snap.config || snap.session.kind !== 'loggedIn') return 'bands.reason.notReady'
    if (!closed) return 'bands.reason.changeInProgress'
    if (speed.running) return 'scan.blockedSpeed'
    return null
  })
</script>

<section class="page" aria-labelledby="scan-title">
  <header>
    <h1 id="scan-title">{t('scan.title')}</h1>
    <p class="lede">{t('scan.intro')}</p>
  </header>
  {#if needsLogin(snap?.session)}<LoginNeeded what="login.needed.scan" />{/if}
  <AutoScan {supported} {tones} {blocked} />
</section>

<style>
  .page {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }
  h1 {
    margin: 0;
    font-size: var(--fs-title);
    line-height: 32px;
  }
  .lede {
    margin-block: 2px 0;
    color: var(--muted);
    max-inline-size: var(--measure);
  }
</style>
