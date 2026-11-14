<script lang="ts">
  import { onMount, type Component } from 'svelte'
  import Activity from '@lucide/svelte/icons/activity'
  import SlidersHorizontal from '@lucide/svelte/icons/sliders-horizontal'
  import Gauge from '@lucide/svelte/icons/gauge'
  import ScanSearch from '@lucide/svelte/icons/scan-search'
  import ChartLine from '@lucide/svelte/icons/chart-line'
  import RouterIcon from '@lucide/svelte/icons/router'
  import Settings2 from '@lucide/svelte/icons/settings-2'
  import Info from '@lucide/svelte/icons/info'
  import { app, type Tab } from '../lib/store.svelte'
  import { t, type MessageKey } from '../lib/i18n'
  import { loginShort, loginState, loginTone } from '../lib/login'
  import { credentials } from '../lib/credentials.svelte'
  import { isolate, networkTypeLabel } from '../lib/format'
  import { loginUptime } from '../lib/uptime'
  import { ensureUpdates, updates } from '../lib/updates.svelte'
  import { scanActive } from '../lib/scan.svelte'
  import { ensureSpeedTest, speed } from '../lib/speedtest.svelte'
  import SignalBars from './ui/SignalBars.svelte'
  import OperatorBadge from './ui/OperatorBadge.svelte'

  const TABS: { id: Tab; label: MessageKey; icon: Component<{ size?: number; 'aria-hidden'?: 'true' }> }[] = [
    { id: 'dashboard', label: 'app.signal', icon: Activity },
    { id: 'bands', label: 'app.bands', icon: SlidersHorizontal },
    { id: 'scan', label: 'app.scan', icon: ScanSearch },
    { id: 'speedtest', label: 'app.speedtest', icon: Gauge },
    { id: 'router', label: 'app.router', icon: RouterIcon },
    { id: 'history', label: 'app.history', icon: ChartLine },
    { id: 'settings', label: 'app.settings', icon: Settings2 },
    { id: 'about', label: 'app.about', icon: Info },
  ]

  const snap = $derived(app.snapshot)
  const loggedIn = $derived(snap?.session.kind === 'loggedIn')
  const digits = $derived(app.settings?.digits ?? 'western')

  const model = $derived(snap?.device?.model ?? t('shell.router'))
  const host = $derived(app.settings?.host ?? '')

  // The status word always comes with its dot, so the colour is never the only signal.
  const status = $derived.by((): { text: string; tone: 'ok' | 'warn' | 'bad' } => {
    if (!snap) return { text: t('app.loading'), tone: 'warn' }
    if (!loggedIn) {
      const s = loginState(snap.session, credentials.status)
      return { text: t(loginShort(s)), tone: loginTone(s) }
    }
    const c = snap.status?.connection
    if (!c) return { text: t('session.loggedIn'), tone: 'ok' }
    if (c.kind === 'other') return { text: t('dashboard.conn.other', { code: c.code }), tone: 'warn' }
    if (c.kind === 'connected')
      return snap.inService ? { text: t('shell.inService'), tone: 'ok' } : { text: t('dashboard.conn.noService'), tone: 'bad' }
    return { text: t(`dashboard.conn.${c.kind}`), tone: c.kind === 'connecting' ? 'warn' : 'bad' }
  })

  // Unknown network types read as a dash, which says nothing here.
  const network = $derived.by(() => {
    const label = loggedIn ? networkTypeLabel(snap?.status?.networkType, snap?.signal) : null
    return label === '—' ? null : label
  })

  let now = $state(Date.now())
  onMount(() => {
    void ensureUpdates()
    void ensureSpeedTest()
    const id = setInterval(() => (now = Date.now()), 1000)
    return () => clearInterval(id)
  })
  const uptime = $derived(loginUptime(loggedIn, now, digits))
  // The link carrying the data: 5G when it is up, otherwise 4G.
  const bars = $derived(loggedIn ? (snap?.status?.nrBars ?? snap?.status?.bars ?? null) : null)
</script>

<aside class="sidebar" aria-label={t('shell.router')}>
  <div class="router">
    <div class="model"><bdi dir="ltr">{model}</bdi></div>
    <div class="meta">
      {#if host}<bdi dir="ltr" class="num">{host}</bdi>{/if}
    </div>
    {#if snap?.status?.operator}<div class="operator"><OperatorBadge operator={{ ...snap.status.operator, homePlmn: snap.device?.homePlmn }} /></div>{/if}
  </div>

  <div class="status" role="status" aria-label={t('shell.status')}>
    <div class="pill {status.tone}">
      <span class="dot" aria-hidden="true"></span>
      <span class="label word">{status.text}</span>
      {#if bars !== null}<span class="signal"><SignalBars value={bars} size={16} /></span>{/if}
    </div>
  </div>
  <!-- Outside the status region: a screen reader shouldn't read the clock out every second. -->
  {#if network || uptime}
    <div class="meta label details">
      {#if network}<bdi>{network}</bdi>{/if}{#if network && uptime}<br />{/if}{#if uptime}<span class="num">{t('shell.loggedInFor', { time: isolate(uptime) })}</span>{/if}
    </div>
  {/if}

  <nav aria-label={t('app.navigation')}>
    {#each TABS as tab (tab.id)}
      {@const Icon = tab.icon}
      <button
        type="button"
        class="item"
        class:foot={tab.id === 'settings'}
        aria-current={app.tab === tab.id ? 'page' : undefined}
        title={t(tab.label)}
        onclick={() => (app.tab = tab.id)}
      >
        <Icon size={16} aria-hidden="true" />
        <span class="label">{t(tab.label)}</span>
        {#if tab.id === 'scan' && scanActive()}
          <span class="update-dot" title={t('scan.running')}></span>
        {:else if tab.id === 'speedtest' && speed.running && !scanActive()}
          <span class="update-dot" title={t('speedtest.testingNow')}></span>
        {:else if tab.id === 'about' && updates.view?.available}
          <span class="update-dot" title={t('updates.available', { version: updates.view.available.version })}></span>
        {/if}
      </button>
    {/each}
  </nav>

</aside>

<style>
  .sidebar {
    inline-size: var(--sidebar-width);
    flex: none;
    display: flex;
    flex-direction: column;
    gap: 18px;
    padding-block: 6px 16px;
    padding-inline: 12px;
    min-block-size: 0;
    overflow-y: auto;
  }
  .router,
  .status,
  .details {
    padding-inline: 6px;
    min-inline-size: 0;
  }
  .details {
    margin-block-start: -12px;
  }
  .model {
    font-family: var(--font-heading);
    font-weight: 700;
    font-size: 15px;
    overflow-wrap: anywhere;
  }
  .meta {
    color: var(--muted);
    font-size: var(--fs-small);
    overflow-wrap: anywhere;
  }
  .operator {
    margin-block-start: 6px;
    font-size: var(--fs-small);
  }
  nav {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .item {
    all: unset;
    box-sizing: border-box;
    flex: none;
    display: flex;
    align-items: center;
    gap: 10px;
    block-size: var(--control-height);
    padding-inline: 10px;
    border-radius: var(--radius-control);
    color: var(--muted);
    cursor: pointer;
    transition:
      background-color var(--t-fast) var(--ease),
      color var(--t-fast) var(--ease);
  }
  /* The app's own pages stay on top; Settings and About wait at the bottom. */
  .item.foot {
    margin-block-start: auto;
  }
  .item:hover:not(:disabled) {
    background: var(--raised);
    color: var(--text);
  }
  .item[aria-current='page'],
  .item[aria-current='page']:hover:not(:disabled) {
    background: var(--active);
    color: var(--text);
    font-weight: 500;
  }
  .item[aria-current='page'] :global(svg) {
    color: var(--accent);
  }
  .update-dot {
    inline-size: 8px;
    block-size: 8px;
    margin-inline-start: auto;
    border-radius: 50%;
    background: var(--accent);
    flex: none;
  }
  .item:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }
  .status {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  /* The first thing to read in the sidebar; its word and dot say the same as its colour. */
  .pill {
    display: flex;
    align-items: center;
    gap: 8px;
    min-block-size: 34px;
    padding-block: 6px;
    padding-inline: 10px;
    border-radius: var(--radius-control);
    font-weight: 600;
    font-size: 14px;
    line-height: 1.3;
  }
  .pill.ok {
    background: var(--pill-positive-bg);
    color: var(--pill-positive-ink);
  }
  .pill.warn {
    background: var(--pill-warning-bg);
    color: var(--pill-warning-ink);
  }
  .pill.bad {
    background: var(--pill-negative-bg);
    color: var(--pill-negative-ink);
  }
  .word {
    flex: 1;
    min-inline-size: 0;
  }
  .signal {
    display: inline-flex;
    flex: none;
  }
  .dot {
    flex: none;
    inline-size: 8px;
    block-size: 8px;
    border-radius: 50%;
    background: currentColor;
  }

  /* Narrow windows: icons only; the names stay as tooltips and accessible names. */
  @media (max-width: 760px) {
    .sidebar {
      inline-size: 60px;
      padding-inline: 8px;
    }
    .router,
    .details {
      display: none;
    }
    .item {
      justify-content: center;
      padding-inline: 0;
    }
    .label {
      position: absolute;
      inline-size: 1px;
      block-size: 1px;
      overflow: hidden;
      clip-path: inset(50%);
      white-space: nowrap;
    }
    .pill {
      justify-content: center;
      padding-inline: 0;
    }
    .signal {
      display: none;
    }
  }
</style>
