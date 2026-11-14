<script lang="ts">
  // The read-only privacy check: what the operator can reach on this router, the devices on the
  // network, and which DNS server answers this PC. Nothing is read until a button is pressed.
  // The result lives in memory (`privacyCheck.svelte.ts`), so leaving the page keeps it; it is
  // never written to disk, and logging out of the router forgets it.
  import type { Component } from 'svelte'
  import Cable from '@lucide/svelte/icons/cable'
  import Cctv from '@lucide/svelte/icons/cctv'
  import CircleQuestionMark from '@lucide/svelte/icons/circle-question-mark'
  import Gamepad2 from '@lucide/svelte/icons/gamepad-2'
  import Laptop from '@lucide/svelte/icons/laptop'
  import Microchip from '@lucide/svelte/icons/microchip'
  import Monitor from '@lucide/svelte/icons/monitor'
  import Printer from '@lucide/svelte/icons/printer'
  import RectangleGoggles from '@lucide/svelte/icons/rectangle-goggles'
  import Rotate3d from '@lucide/svelte/icons/rotate-3d'
  import RouterIcon from '@lucide/svelte/icons/router'
  import ShieldCheck from '@lucide/svelte/icons/shield-check'
  import Smartphone from '@lucide/svelte/icons/smartphone'
  import Speaker from '@lucide/svelte/icons/speaker'
  import Tablet from '@lucide/svelte/icons/tablet'
  import Tv from '@lucide/svelte/icons/tv'
  import Watch from '@lucide/svelte/icons/watch'
  import Wifi from '@lucide/svelte/icons/wifi'
  import { api } from '../lib/backend'
  import { app } from '../lib/store.svelte'
  import { t, type MessageKey } from '../lib/i18n'
  import { errorText } from '../lib/errors'
  import { formatNumber } from '../lib/format'
  import { deviceCounts, deviceKind, maskAddress, readFailure, type DeviceKind } from '../lib/privacy'
  import {
    beginPrivacyRun,
    forgetPrivacyCheck,
    keepPrivacyReport,
    privacy,
    routerKey,
    sameLogin,
  } from '../lib/privacyCheck.svelte'
  import InfoTip from './ui/InfoTip.svelte'

  // A picture for the app's guess at what each device is.
  const KIND_ICON: Record<DeviceKind, Component<{ size?: number | string }>> = {
    phone: Smartphone,
    tablet: Tablet,
    computer: Monitor,
    laptop: Laptop,
    tv: Tv,
    console: Gamepad2,
    vr: RectangleGoggles,
    tracker: Rotate3d,
    printer: Printer,
    watch: Watch,
    speaker: Speaker,
    camera: Cctv,
    network: RouterIcon,
    chip: Microchip,
    unknown: CircleQuestionMark,
  }

  // Whether a check is running is in the store: this card can be left and opened again meanwhile.
  const busy = $derived(privacy.running)
  let error: string | null = $state(null)
  let dnsBusy = $state(false)
  let dnsError: string | null = $state(null)

  const digits = $derived(app.settings?.digits ?? 'western')
  const sessionKind = $derived(app.snapshot?.session.kind ?? null)
  const loggedIn = $derived(sessionKind === 'loggedIn')
  // What was read belongs to the login it was read in. A short reconnect keeps it; a logout, a
  // refused password or another router forgets it (the store watches that on every page).
  const stillIn = $derived(sameLogin(sessionKind))
  const report = $derived(stillIn ? privacy.report : null)
  const dns = $derived(privacy.dns)
  // Already known from the router's internet address: no new request. The type speaks for IPv4
  // only, so an IPv6 address gets its own line and no assurance.
  const wanKind = $derived(loggedIn ? (app.snapshot?.wan?.kind ?? null) : null)
  const hasIpv6 = $derived(loggedIn && (app.snapshot?.wan?.ipv6 ?? false))
  const management = $derived(report?.management ?? null)
  const managementFailure = $derived(report ? readFailure(report.remoteManagement) : null)
  const devicesFailure = $derived(report ? readFailure(report.devicesRead) : null)
  const counts = $derived(deviceCounts(report?.devices ?? []))
  const anyLink = $derived((report?.devices ?? []).some((d) => d.link !== null))
  // What the router's mark means isn't confirmed, so it counts only when one device has it.
  const oneThisPc = $derived((report?.devices ?? []).filter((d) => d.thisDevice).length === 1)

  function num(n: number): string {
    return formatNumber(n, { digits })
  }

  async function run() {
    if (privacy.running) return
    privacy.running = true
    error = null
    // An answer that arrives after a logout or a change of router belongs to neither: it is
    // dropped, and the card stays at "not checked yet".
    const started = beginPrivacyRun(routerKey(app.settings))
    try {
      keepPrivacyReport(await api.privacyCheck(), started, routerKey(app.settings))
    } catch (e) {
      error = errorText(e)
    } finally {
      privacy.running = false
    }
  }

  async function checkDns() {
    dnsBusy = true
    dnsError = null
    try {
      privacy.dns = await api.dnsCheck()
      privacy.showResolver = false
    } catch (e) {
      dnsError = errorText(e)
    } finally {
      dnsBusy = false
    }
  }

  $effect(() => {
    if (!stillIn) forgetPrivacyCheck()
  })
</script>

<section class="card privacy" aria-labelledby="privacy-title">
  <div class="head">
    <h2 id="privacy-title"><ShieldCheck size={18} aria-hidden="true" />{t('privacy.title')}</h2>
    <button type="button" onclick={run} disabled={busy || !loggedIn}>
      {busy ? t('privacy.running') : report ? t('privacy.runAgain') : t('privacy.run')}
    </button>
  </div>
  <p class="lede">{t('privacy.body')}</p>
  {#if !loggedIn && !report}<p class="muted">{t('pageScripts.needLogin')}</p>{/if}
  {#if error}<p class="err" role="alert">{error}</p>{/if}

  <div class="parts">
    <div class="part" data-testid="privacy-management">
      <h3>
        {t('privacy.management.title')}
        <InfoTip label={t('metrics.whatIs', { name: 'TR-069' })} text={t('privacy.management.help')} />
      </h3>
      {#if !report}
        <p class="muted">{t('privacy.notRun')}</p>
      {:else if managementFailure}
        <p class="refused">{t(managementFailure.key, managementFailure.params)}</p>
      {:else}
        <!-- The answer first: on or off, said only when the router's own field said it. What it
             means is behind the tip. -->
        {#if management?.enabled === true}
          <p>{t('privacy.management.on')}</p>
        {:else if management?.enabled === false}
          <p>{t('privacy.management.off')}</p>
        {:else if management?.serverSet === true}
          <p class="muted">{t('privacy.management.switchUnknown')}</p>
        {/if}
        {#if management?.serverHost}
          <p class="line">
            <span class="muted">{t('privacy.management.server')}</span>
            <bdi dir="ltr" class="num">{management.serverHost}</bdi>
          </p>
        {:else if management?.serverSet === true}
          <p>{t('privacy.management.setUnnamed')}</p>
        {:else if management?.serverSet === false}
          <p>{t('privacy.management.none')}</p>
        {:else}
          <p>{t('privacy.management.notFound')}</p>
        {/if}
      {/if}
    </div>

    <div class="part" data-testid="privacy-reach">
      <h3>{t('privacy.reach.title')}</h3>
      {#if wanKind === 'public'}
        <p>{t('privacy.reach.public')}</p>
      {:else if wanKind}
        <p>{t('privacy.reach.shared')}</p>
      {:else}
        <p class="muted">{t('privacy.reach.unknown')}</p>
      {/if}
      {#if hasIpv6}<p class="muted">{t('privacy.reach.ipv6')}</p>{/if}
    </div>

    <div class="part" data-testid="privacy-dns">
      <h3>
        {t('privacy.dns.title')}
        <InfoTip label={t('metrics.whatIs', { name: 'DNS' })} text={t('privacy.dns.help')} />
      </h3>
      <!-- The one request that leaves the network is said before the button. -->
      <p class="muted">{t('privacy.dns.body')}</p>
      <div class="row">
        <button type="button" onclick={checkDns} disabled={dnsBusy}>
          {dnsBusy ? t('privacy.dns.checking') : t('privacy.dns.check')}
        </button>
      </div>
      {#if dnsError}
        <p class="err" role="alert">{dnsError}</p>
      {:else if dns?.resolver}
        <p class="line">
          <span>{t('privacy.dns.answeredBy')}</span>
          <bdi dir="ltr" class="num" data-testid="privacy-dns-address">{privacy.showResolver ? dns.resolver : maskAddress(dns.resolver)}</bdi>
          <button
            type="button"
            class="link"
            aria-pressed={privacy.showResolver}
            onclick={() => (privacy.showResolver = !privacy.showResolver)}
          >
            {privacy.showResolver ? t('network.wan.hideFull') : t('network.wan.showFull')}
          </button>
        </p>
      {:else if dns}
        <p>{t('privacy.dns.none')}</p>
      {/if}
    </div>
  </div>

  <div class="devices" data-testid="privacy-devices">
    <h3>{t('privacy.devices.title')}</h3>
    {#if !report}
      <p class="muted">{t('privacy.notRun')}</p>
    {:else if devicesFailure}
      <p class="refused">{t(devicesFailure.key, devicesFailure.params)}</p>
    {:else if report.devices.length === 0}
      <p>{t('privacy.devices.none')}</p>
    {:else}
      <!-- The two lists don't mean the same thing: say which one the router gave. -->
      <p class="muted">
        {report.devicesSource === 'wifi' ? t('privacy.devices.fromWifi') : t('privacy.devices.fromLan')}
      </p>
      <p class="summary" data-testid="privacy-device-summary">
        <span>{t('privacy.devices.listed', { total: num(counts.total) })}</span>
        {#if counts.online !== null}<span>{t('privacy.devices.online', { online: num(counts.online) })}</span>{/if}
        <span>
          {t('privacy.devices.private', { count: num(counts.privateMac) })}
          <InfoTip
            label={t('metrics.whatIs', { name: t('privacy.devices.privateCol') })}
            text={t('privacy.devices.privateHelp')}
          />
        </span>
      </p>
      <div class="row">
        <button
          type="button"
          aria-expanded={privacy.showDevices}
          onclick={() => (privacy.showDevices = !privacy.showDevices)}
        >
          {privacy.showDevices ? t('privacy.devices.hide') : t('privacy.devices.show')}
        </button>
      </div>
      {#if privacy.showDevices}
        <div class="table-wrap">
          <table>
            <thead>
              <tr>
                <th scope="col">
                  <span class="with-tip">
                    {t('privacy.devices.name')}
                    <InfoTip
                      label={t('metrics.whatIs', { name: t('privacy.devices.name') })}
                      text={t('privacy.devices.nameHelp')}
                    />
                  </span>
                </th>
                {#if anyLink}<th scope="col">{t('privacy.devices.link')}</th>{/if}
                <th scope="col">{t('privacy.devices.mac')}</th>
                <th scope="col">{t('privacy.devices.ip')}</th>
                {#if counts.online !== null}<th scope="col">{t('privacy.devices.state')}</th>{/if}
                <th scope="col">{t('privacy.devices.privateCol')}</th>
              </tr>
            </thead>
            <tbody>
              <!-- A name is whatever its device chose to call itself: plain text only, in its own direction. -->
              {#each report.devices as d}
                {@const kind = deviceKind(d.name)}
                {@const KindIcon = KIND_ICON[kind]}
                {@const kindLabel = t(`privacy.devices.kind.${kind}` as MessageKey)}
                <tr>
                  <td>
                    <span class="who">
                      <span class="kind" role="img" aria-label={kindLabel} title={kindLabel}><KindIcon size={18} /></span>
                      {#if d.name}<bdi>{d.name}</bdi>{:else}<span class="muted">{t('privacy.devices.unnamed')}</span>{/if}
                      {#if d.thisDevice && oneThisPc}<span class="badge">{t('privacy.devices.thisPc')}</span>{/if}
                    </span>
                  </td>
                  {#if anyLink}
                    <td>
                      {#if d.link === 'wifi'}
                        <span class="how"><Wifi size={16} aria-hidden="true" /><span>{t('privacy.devices.wifi')}</span></span>
                      {:else if d.link === 'cable'}
                        <span class="how"><Cable size={16} aria-hidden="true" /><span>{t('privacy.devices.cable')}</span></span>
                      {:else}—{/if}
                    </td>
                  {/if}
                  <td><bdi dir="ltr" class="num">{d.mac}</bdi></td>
                  <td>{#if d.ip}<bdi dir="ltr" class="num">{d.ip}</bdi>{:else}—{/if}</td>
                  {#if counts.online !== null}
                    <td>{d.active === null ? '—' : d.active ? t('privacy.devices.yes') : t('privacy.devices.no')}</td>
                  {/if}
                  <td>{d.privateMac ? t('privacy.devices.yes') : t('privacy.devices.no')}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}
    {/if}
  </div>
</section>

<style>
  .card {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius-card);
    padding-block: var(--card-pad-block);
    padding-inline: var(--card-pad-inline);
    display: flex;
    flex-direction: column;
    gap: 10px;
    min-inline-size: 0;
  }
  .head {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 8px 16px;
  }
  p {
    margin: 0;
  }
  .lede {
    max-inline-size: var(--measure);
  }
  h3 {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 0;
    font-size: var(--fs-body);
  }
  /* The parts share the row evenly, so a wide window has no empty half. */
  .parts {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(240px, 1fr));
    gap: var(--gap);
  }
  .part {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding-block: 10px;
    padding-inline: 12px;
    background: var(--raised);
    border-radius: var(--radius-control);
    min-inline-size: 0;
  }
  .devices {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .line {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 2px 8px;
  }
  .line bdi {
    overflow-wrap: anywhere;
  }
  .summary {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 24px;
  }
  .summary span,
  .with-tip {
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }
  .row {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  .muted {
    color: var(--muted);
  }
  /* A refusal is the router's answer, not a fault in the app. */
  .refused {
    color: var(--q-fair);
  }
  .err {
    color: var(--q-poor);
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
  .table-wrap {
    overflow-x: auto;
  }
  table {
    inline-size: 100%;
    border-collapse: collapse;
  }
  th,
  td {
    text-align: start;
    padding-block: 6px;
    padding-inline: 0 16px;
    border-block-end: 1px solid var(--border);
    vertical-align: middle;
  }
  th {
    color: var(--muted);
    font-weight: 500;
    font-size: var(--fs-small);
  }
  td bdi {
    overflow-wrap: anywhere;
  }
  .who,
  .how {
    display: inline-flex;
    align-items: center;
    gap: 8px;
  }
  .how {
    gap: 6px;
  }
  .kind {
    display: inline-flex;
    color: var(--muted);
    flex: none;
  }
  .how :global(svg) {
    color: var(--muted);
    flex: none;
  }
  .badge {
    padding-block: 1px;
    padding-inline: 8px;
    border: 1px solid var(--border);
    border-radius: var(--radius-control);
    background: var(--raised);
    font-size: var(--fs-small);
    white-space: nowrap;
  }
</style>
