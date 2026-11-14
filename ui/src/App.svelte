<script lang="ts">
  import { onMount } from 'svelte'
  import { api } from './lib/backend'
  import { app, startStore, pushToast, updateSettings } from './lib/store.svelte'
  import { t } from './lib/i18n'
  import { applyTheme, followSystemTheme } from './lib/theme'
  import { applyUiScale, uiScaleFromWheel, uiScaleShortcut, UI_SCALE_DEFAULT } from './lib/uiScale'
  import { errorText } from './lib/errors'
  import TitleBar from './components/TitleBar.svelte'
  import Sidebar from './components/Sidebar.svelte'
  import Dashboard from './components/Dashboard.svelte'
  import Toasts from './components/Toasts.svelte'
  import BandPanel from './components/BandPanel.svelte'
  import ScanPage from './components/ScanPage.svelte'
  import SpeedTest from './components/SpeedTest.svelte'
  import HistoryPage from './components/HistoryPage.svelte'
  import RouterPage from './components/RouterPage.svelte'
  import SafeApplyBanner from './components/SafeApplyBanner.svelte'
  import ForeignJournalNotice from './components/ForeignJournalNotice.svelte'
  import DataSwitchNotice from './components/DataSwitchNotice.svelte'
  import IpRefreshBanner from './components/IpRefreshBanner.svelte'
  import IpRefreshDialog from './components/IpRefreshDialog.svelte'
  import UnsupportedRouter from './components/UnsupportedRouter.svelte'
  import Settings from './components/Settings.svelte'
  import About from './components/About.svelte'
  import Onboarding from './components/Onboarding.svelte'
  import LoginPanel from './components/LoginPanel.svelte'
  import { refreshCred } from './lib/credentials.svelte'
  import { onboardingOpen } from './lib/login'

  const onboarding = $derived(onboardingOpen(app.settings, app.snapshot))

  // The stored look; Settings previews unsaved choices itself and restores this when it closes.
  $effect(() => applyTheme(app.settings?.theme, app.settings?.palette, app.settings?.design))
  // The Spectrum design fills the window; Classic keeps its fixed size.
  $effect(() => applyUiScale(app.settings?.uiScale, app.settings?.design === 'spectrum'))

  // Each page opens at its top, not at wherever the last page was scrolled to.
  let scroller: HTMLDivElement | undefined = $state()
  $effect(() => {
    void app.tab
    if (scroller) scroller.scrollTop = 0
  })

  // A login that ended without a session may have left (or cleared) the rejected-password mark.
  const sessionKind = $derived(app.snapshot?.session.kind)
  $effect(() => {
    const k = sessionKind
    if (k === 'badCredentials' || k === 'lockedOut' || k === 'loginRejected' || k === 'notConfigured' || k === 'loggedOut')
      void refreshCred()
  })

  const storedScale = () => app.settings?.uiScale ?? UI_SCALE_DEFAULT
  function setScale(next: number) {
    if (next === storedScale()) return
    updateSettings((s) => ({ ...s, uiScale: next })).catch((err: unknown) => pushToast(errorText(err), 'error'))
  }

  function onKeydown(e: KeyboardEvent) {
    if (!app.settings) return
    const next = uiScaleShortcut(e, storedScale())
    if (next === null) return
    e.preventDefault()
    setScale(next)
  }

  // One step per wheel gesture burst, however fine-grained the wheel or touchpad is.
  const WHEEL_STEP_MS = 150
  let lastWheel = 0
  function onWheel(e: WheelEvent) {
    if (!app.settings) return
    const next = uiScaleFromWheel(e, storedScale())
    if (next === null) return
    e.preventDefault()
    if (e.timeStamp - lastWheel < WHEEL_STEP_MS) return
    lastWheel = e.timeStamp
    setScale(next)
  }

  onMount(() => {
    let dispose: (() => void) | undefined
    let gone = false
    const offTheme = followSystemTheme()
    // Not passive, so Ctrl + wheel can't also scroll the page.
    window.addEventListener('wheel', onWheel, { passive: false })
    void refreshCred()
    // startStore loads settings (and applies the language), the first snapshot and the event streams.
    startStore()
      .then((d) => (gone ? d() : (dispose = d)))
      .catch((e: unknown) => pushToast(t('app.error', { message: String(e) }), 'error'))

    const sendVisible = () => void api.setVisible(!document.hidden).catch(() => {})
    sendVisible()
    document.addEventListener('visibilitychange', sendVisible)
    return () => {
      gone = true
      dispose?.()
      offTheme()
      window.removeEventListener('wheel', onWheel)
      document.removeEventListener('visibilitychange', sendVisible)
    }
  })
</script>

<svelte:window onkeydown={onKeydown} />

<!-- The guide is modal: nothing behind it can take focus or clicks. -->
<div class="shell" inert={onboarding}>
  <TitleBar />
  <div class="body">
    <Sidebar />
    <main>
      <div class="scroll" bind:this={scroller}>
        <div class="stack">
          <ForeignJournalNotice />
          <DataSwitchNotice />
          <UnsupportedRouter />
          <SafeApplyBanner />
          <SafeApplyBanner subject="apn" />
          <!-- A Refresh IP run stays in view on every page, like a band change. -->
          <IpRefreshBanner />
          <!-- On every page: the router question and a wrong password must never stay hidden.
               Settings has its own Router connection panel, and the guide asks for the password itself. -->
          <LoginPanel variant="banner" />
          {#key app.tab}
            <div class="page">
              {#if app.tab === 'dashboard'}
                <Dashboard />
              {:else if app.tab === 'bands'}
                <BandPanel />
              {:else if app.tab === 'scan'}
                <ScanPage />
              {:else if app.tab === 'speedtest'}
                <SpeedTest />
              {:else if app.tab === 'router'}
                <RouterPage />
              {:else if app.tab === 'history'}
                <HistoryPage />
              {:else if app.tab === 'settings'}
                <Settings />
              {:else}
                <About />
              {/if}
            </div>
          {/key}
        </div>
      </div>
      <!-- Settings' save bar lives here, outside the scrolling page, so it never covers content. -->
      <div id="page-dock"></div>
    </main>
  </div>
</div>
{#if onboarding}
  <Onboarding />
{/if}
<IpRefreshDialog />
<Toasts />

<style>
  .shell {
    block-size: 100%;
    display: flex;
    flex-direction: column;
    background: var(--ground);
  }
  .body {
    flex: 1;
    display: flex;
    min-block-size: 0;
  }
  main {
    flex: 1;
    min-inline-size: 0;
    display: flex;
    flex-direction: column;
  }
  .scroll {
    flex: 1;
    min-block-size: 0;
    overflow-y: auto;
    /* The scrollbar's room is always kept, so pages don't shift sideways when one appears. */
    scrollbar-gutter: stable;
    padding-block: 8px 24px;
    padding-inline: var(--page-gutter);
  }
  .page {
    min-inline-size: 0;
    animation: page-in var(--t-enter) var(--ease);
  }
  .stack {
    display: flex;
    flex-direction: column;
    gap: var(--gap);
    inline-size: 100%;
    max-inline-size: var(--content-max);
    margin-inline: auto;
  }
</style>
