import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))
vi.mock('../lib/backend', () => ({
  api: { privacyCheck: vi.fn(), dnsCheck: vi.fn(), saveSettings: vi.fn() },
}))

import { api } from '../lib/backend'
import { t } from '../lib/i18n'
import { forgetPrivacyCheck, privacy } from '../lib/privacyCheck.svelte'
import { app, ingestSnapshot, updateSettings } from '../lib/store.svelte'
import { settingsFixture, snap } from '../test-fixtures'
import type { AppSnapshot } from '../lib/bindings/AppSnapshot'
import type { LanDevice } from '../lib/bindings/LanDevice'
import type { PrivacyReport } from '../lib/bindings/PrivacyReport'
import type { WanKind } from '../lib/bindings/WanKind'
import PrivacyCheck from './PrivacyCheck.svelte'

function device(o: Partial<LanDevice> & { mac: string }): LanDevice {
  return { name: null, ip: null, active: true, privateMac: false, link: null, thisDevice: false, ...o }
}

// Made-up devices in the documentation MAC range.
const REPORT: PrivacyReport = {
  remoteManagement: { outcome: 'ok', code: null },
  management: { serverHost: 'acs.example.net', serverSet: true, enabled: null },
  devicesRead: { outcome: 'ok', code: null },
  devicesSource: 'lan',
  devices: [
    device({ name: 'Office PC', mac: '00:00:5E:00:53:10', ip: '192.168.8.100', link: 'cable', thisDevice: true }),
    device({ name: 'iPhone', mac: 'DA:A1:19:00:53:12', ip: '192.168.8.102', privateMac: true, link: 'wifi' }),
    device({ mac: '00:00:5E:00:53:14', active: false }),
  ],
}

type SessionKind = 'loggedIn' | 'loggedOut' | 'reconnecting'

function session(kind: SessionKind, wanKind: WanKind | null = 'cgnat', ipv6 = false) {
  app.snapshot = {
    session: { kind },
    wan: { ipv4: null, range: null, kind: wanKind, ipv6 },
  } as unknown as AppSnapshot
}

/** A first run on a card that has no result yet. */
async function run(report: PrivacyReport = REPORT) {
  forgetPrivacyCheck()
  vi.mocked(api.privacyCheck).mockResolvedValue(report)
  render(PrivacyCheck)
  await fireEvent.click(screen.getByRole('button', { name: t('privacy.run') }))
  await waitFor(() => screen.getByRole('button', { name: t('privacy.runAgain') }))
}

describe('PrivacyCheck', () => {
  beforeEach(() => session('loggedIn'))
  afterEach(() => {
    cleanup()
    vi.clearAllMocks()
    forgetPrivacyCheck()
    privacy.running = false
    privacy.dns = null
    privacy.showResolver = false
    app.snapshot = null
    app.settings = null
  })

  it('keeps the result when the page is left and opened again', async () => {
    await run()
    await fireEvent.click(screen.getByRole('button', { name: t('privacy.devices.show') }))
    expect(screen.getByText('Office PC')).toBeTruthy()
    // Leaving the Router page unmounts the card; coming back mounts a new one.
    cleanup()
    render(PrivacyCheck)
    expect(screen.getByText('Office PC')).toBeTruthy()
    expect(within(screen.getByTestId('privacy-management')).getByText('acs.example.net')).toBeTruthy()
    expect(api.privacyCheck).toHaveBeenCalledOnce()
  })

  it('keeps the result through a short reconnect, and forgets it after a logout', async () => {
    await run()
    session('reconnecting')
    await waitFor(() => expect((screen.getByRole('button', { name: t('privacy.runAgain') }) as HTMLButtonElement).disabled).toBe(true))
    expect(within(screen.getByTestId('privacy-management')).getByText('acs.example.net')).toBeTruthy()
    session('loggedOut')
    await waitFor(() => expect(screen.queryByText('acs.example.net')).toBeNull())
    // Logging in again doesn't bring the old result back.
    session('loggedIn')
    await waitFor(() => screen.getByRole('button', { name: t('privacy.run') }))
    expect(screen.queryByText('acs.example.net')).toBeNull()
  })

  it('forgets the result on a logout that happens while another page is open', async () => {
    app.settings = settingsFixture()
    await run()
    await fireEvent.click(screen.getByRole('button', { name: t('privacy.devices.show') }))
    // The user goes to another page, logs out there, and logs in again.
    cleanup()
    ingestSnapshot(snap({ session: { kind: 'loggedOut' } }))
    ingestSnapshot(snap({ session: { kind: 'loggedIn' } }))
    render(PrivacyCheck)
    expect(screen.getByRole('button', { name: t('privacy.run') })).toBeTruthy()
    expect(screen.queryByText('acs.example.net')).toBeNull()
    expect(screen.queryByText('Office PC')).toBeNull()
  })

  it('keeps the result through a reconnect on another page, and forgets it for another router', async () => {
    app.settings = settingsFixture()
    await run()
    cleanup()
    ingestSnapshot(snap({ session: { kind: 'reconnecting', reason: 'timeout', retryInS: 5 } }))
    ingestSnapshot(snap({ session: { kind: 'loggedIn' } }))
    expect(privacy.report).not.toBeNull()
    // Settings now name another router: what was read isn't about it.
    app.settings = { ...settingsFixture(), host: '192.168.1.1' }
    ingestSnapshot(snap({ session: { kind: 'loggedIn' } }))
    expect(privacy.report).toBeNull()
  })

  /** Starts a check whose answer the test hands over later. */
  async function startSlowCheck(): Promise<(r: PrivacyReport) => void> {
    forgetPrivacyCheck()
    let finish!: (r: PrivacyReport) => void
    vi.mocked(api.privacyCheck).mockReturnValue(new Promise((resolve) => (finish = resolve)))
    render(PrivacyCheck)
    await fireEvent.click(screen.getByRole('button', { name: t('privacy.run') }))
    return finish
  }
  const idleAgain = () =>
    waitFor(() => expect((screen.getByRole('button', { name: t('privacy.run') }) as HTMLButtonElement).disabled).toBe(false))

  it('drops an answer that arrives after its login ended, even when a new login has begun', async () => {
    app.settings = settingsFixture()
    ingestSnapshot(snap({ session: { kind: 'loggedIn' } }))
    const finish = await startSlowCheck()
    // The user logs out and connects again while the router is still answering.
    ingestSnapshot(snap({ session: { kind: 'loggedOut' } }))
    ingestSnapshot(snap({ session: { kind: 'connecting' } }))
    ingestSnapshot(snap({ session: { kind: 'loggedIn' } }))
    finish(REPORT)
    await idleAgain()
    expect(privacy.report).toBeNull()
    expect(screen.queryByText('acs.example.net')).toBeNull()
  })

  it('drops an answer when the settings came to name another router while it was read', async () => {
    app.settings = settingsFixture()
    ingestSnapshot(snap({ session: { kind: 'loggedIn' } }))
    const finish = await startSlowCheck()
    app.settings = { ...settingsFixture(), host: '192.168.1.1' }
    finish(REPORT)
    await idleAgain()
    expect(privacy.report).toBeNull()
  })

  it('forgets the result as soon as the saved settings name another router', async () => {
    app.settings = settingsFixture()
    ingestSnapshot(snap({ session: { kind: 'loggedIn' } }))
    await run()
    expect(privacy.report).not.toBeNull()
    vi.mocked(api.saveSettings).mockResolvedValue(undefined)
    await updateSettings((s) => ({ ...s, host: '192.168.1.1' }))
    // No snapshot has arrived since the save.
    expect(privacy.report).toBeNull()
  })

  it('does not start a second check when the page is left and opened again during one', async () => {
    const finish = await startSlowCheck()
    cleanup()
    render(PrivacyCheck)
    const again = screen.getByRole('button', { name: t('privacy.running') }) as HTMLButtonElement
    expect(again.disabled).toBe(true)
    await fireEvent.click(again)
    expect(api.privacyCheck).toHaveBeenCalledOnce()
    finish(REPORT)
    await waitFor(() => screen.getByRole('button', { name: t('privacy.runAgain') }))
  })

  it('marks this PC only when the router marks exactly one device', async () => {
    await run({
      ...REPORT,
      devices: [
        device({ name: 'Office PC', mac: '00:00:5E:00:53:10', thisDevice: true }),
        device({ name: 'Other PC', mac: '00:00:5E:00:53:11', thisDevice: true }),
      ],
    })
    await fireEvent.click(screen.getByRole('button', { name: t('privacy.devices.show') }))
    expect(screen.getByText('Office PC')).toBeTruthy()
    expect(screen.queryByText(t('privacy.devices.thisPc'))).toBeNull()
  })

  it('says whether remote management is switched on, off, or that the router did not say', async () => {
    const withSwitch = (enabled: boolean | null): PrivacyReport => ({
      ...REPORT,
      management: { serverHost: 'acs.example.net', serverSet: true, enabled },
    })
    await run(withSwitch(true))
    let tile = within(screen.getByTestId('privacy-management'))
    expect(tile.getByText(t('privacy.management.on'))).toBeTruthy()
    expect(tile.queryByText(t('privacy.management.switchUnknown'))).toBeNull()
    cleanup()
    await run(withSwitch(false))
    tile = within(screen.getByTestId('privacy-management'))
    expect(tile.getByText(t('privacy.management.off'))).toBeTruthy()
    expect(tile.queryByText(t('privacy.management.on'))).toBeNull()
    cleanup()
    await run(withSwitch(null))
    tile = within(screen.getByTestId('privacy-management'))
    expect(tile.getByText(t('privacy.management.switchUnknown'))).toBeTruthy()
    expect(tile.queryByText(t('privacy.management.on'))).toBeNull()
    expect(tile.queryByText(t('privacy.management.off'))).toBeNull()
  })

  it('gives each device a kind, how it is connected, and marks this PC', async () => {
    await run()
    const block = within(screen.getByTestId('privacy-devices'))
    await fireEvent.click(block.getByRole('button', { name: t('privacy.devices.show') }))
    const rows = block.getAllByRole('row').slice(1)
    expect(rows).toHaveLength(3)
    const [pc, phone, unnamed] = rows.map((r) => within(r))
    expect(pc.getByTitle(t('privacy.devices.kind.computer'))).toBeTruthy()
    expect(pc.getByText(t('privacy.devices.cable'))).toBeTruthy()
    expect(pc.getByText(t('privacy.devices.thisPc'))).toBeTruthy()
    expect(phone.getByTitle(t('privacy.devices.kind.phone'))).toBeTruthy()
    expect(phone.getByText(t('privacy.devices.wifi'))).toBeTruthy()
    expect(phone.queryByText(t('privacy.devices.thisPc'))).toBeNull()
    // No name and no connection word: an unknown device, and nothing made up about its link.
    expect(unnamed.getByTitle(t('privacy.devices.kind.unknown'))).toBeTruthy()
    expect(unnamed.getByText(t('privacy.devices.unnamed'))).toBeTruthy()
    expect(unnamed.queryByText(t('privacy.devices.wifi'))).toBeNull()
    expect(unnamed.queryByText(t('privacy.devices.cable'))).toBeNull()
  })

  it('reads nothing until its button is pressed', () => {
    render(PrivacyCheck)
    expect(api.privacyCheck).not.toHaveBeenCalled()
    expect(api.dnsCheck).not.toHaveBeenCalled()
  })

  it('is off while logged out', () => {
    session('loggedOut')
    render(PrivacyCheck)
    expect((screen.getByRole('button', { name: t('privacy.run') }) as HTMLButtonElement).disabled).toBe(true)
    expect(screen.getByText(t('pageScripts.needLogin'))).toBeTruthy()
  })

  it('shows the management server and never claims it is switched on', async () => {
    await run()
    const tile = within(screen.getByTestId('privacy-management'))
    expect(tile.getByText('acs.example.net')).toBeTruthy()
    expect(tile.getByText(t('privacy.management.server'))).toBeTruthy()
    expect(tile.getByText(t('privacy.management.switchUnknown'))).toBeTruthy()
  })

  it('keeps each part to its answer, with the explanation behind a tip', async () => {
    vi.mocked(api.dnsCheck).mockResolvedValue({ resolver: '203.0.113.5' })
    await run()
    // Remote management: the switch and the server, nothing else.
    expect(screen.getByTestId('privacy-management').querySelectorAll('p')).toHaveLength(2)

    const dns = screen.getByTestId('privacy-dns')
    expect(within(dns).getByRole('button', { name: t('metrics.whatIs', { name: 'DNS' }) })).toBeTruthy()
    // That it asks the internet is said before the button is pressed.
    expect(within(dns).getByText(t('privacy.dns.body'))).toBeTruthy()
    await fireEvent.click(within(dns).getByRole('button', { name: t('privacy.dns.check') }))
    await waitFor(() => within(dns).getByTestId('privacy-dns-address'))
    // The answer is the only line the check adds.
    expect(dns.querySelectorAll('p')).toHaveLength(2)

    // Devices: which list it is and the counts, and no note above the table.
    const devices = screen.getByTestId('privacy-devices')
    await fireEvent.click(within(devices).getByRole('button', { name: t('privacy.devices.show') }))
    expect(devices.querySelectorAll('p')).toHaveLength(2)
  })

  it('says so when no management server is set, or none was found', async () => {
    await run({ ...REPORT, management: { serverHost: null, serverSet: false, enabled: null } })
    expect(within(screen.getByTestId('privacy-management')).getByText(t('privacy.management.none'))).toBeTruthy()
    cleanup()
    await run({ ...REPORT, management: { serverHost: null, serverSet: null, enabled: null } })
    expect(within(screen.getByTestId('privacy-management')).getByText(t('privacy.management.notFound'))).toBeTruthy()
    cleanup()
    await run({ ...REPORT, management: { serverHost: null, serverSet: true, enabled: null } })
    expect(within(screen.getByTestId('privacy-management')).getByText(t('privacy.management.setUnnamed'))).toBeTruthy()
  })

  it('shows a refused read as refused, with the router code, and still shows the other read', async () => {
    await run({ ...REPORT, remoteManagement: { outcome: 'refused', code: 100003 }, management: null })
    const tile = within(screen.getByTestId('privacy-management'))
    expect(tile.getByText(t('privacy.read.refusedCode', { code: '100003' }))).toBeTruthy()
    expect(tile.queryByText(t('privacy.management.server'))).toBeNull()
    expect(within(screen.getByTestId('privacy-devices')).getByRole('button', { name: t('privacy.devices.show') })).toBeTruthy()
  })

  it('shows a refused device list as refused and offers no list', async () => {
    await run({ ...REPORT, devicesRead: { outcome: 'notOffered', code: 100002 }, devicesSource: null, devices: [] })
    const block = within(screen.getByTestId('privacy-devices'))
    expect(block.getByText(t('privacy.read.notOfferedCode', { code: '100002' }))).toBeTruthy()
    expect(block.queryByRole('button', { name: t('privacy.devices.show') })).toBeNull()
  })

  it('keeps device names and addresses off the screen until asked, and hides them again', async () => {
    await run()
    const block = within(screen.getByTestId('privacy-devices'))
    expect(screen.queryByText('Office PC')).toBeNull()
    expect(screen.queryByText('00:00:5E:00:53:10')).toBeNull()
    expect(screen.queryByText('192.168.8.100')).toBeNull()
    // The counts alone are shown: 3 listed, 2 online, 1 private address.
    expect(block.getByTestId('privacy-device-summary').textContent).toContain('3')
    expect(block.getByTestId('privacy-device-summary').textContent).toContain('2')

    await fireEvent.click(block.getByRole('button', { name: t('privacy.devices.show') }))
    expect(screen.getByText('Office PC')).toBeTruthy()
    expect(screen.getByText('DA:A1:19:00:53:12')).toBeTruthy()
    expect(screen.getByText('192.168.8.100')).toBeTruthy()
    expect(screen.getByText(t('privacy.devices.unnamed'))).toBeTruthy()

    await fireEvent.click(block.getByRole('button', { name: t('privacy.devices.hide') }))
    expect(screen.queryByText('Office PC')).toBeNull()
  })

  it('says which list the router gave, since they do not mean the same thing', async () => {
    await run()
    const block = within(screen.getByTestId('privacy-devices'))
    expect(block.getByText(t('privacy.devices.fromLan'))).toBeTruthy()
    expect(block.queryByText(t('privacy.devices.fromWifi'))).toBeNull()
    cleanup()
    await run({
      ...REPORT,
      devicesSource: 'wifi',
      devices: REPORT.devices.map((d) => ({ ...d, active: null })),
    })
    const wifi = within(screen.getByTestId('privacy-devices'))
    expect(wifi.getByText(t('privacy.devices.fromWifi'))).toBeTruthy()
    expect(wifi.queryByText(t('privacy.devices.fromLan'))).toBeNull()
  })

  it('shows a device name as plain text, whatever it holds', async () => {
    const hostile = '<img src=x onerror="window.pwned=1"><b>bold</b>'
    await run({
      ...REPORT,
      devices: [device({ name: hostile, mac: '00:00:5E:00:53:10' })],
    })
    const block = screen.getByTestId('privacy-devices')
    await fireEvent.click(within(block).getByRole('button', { name: t('privacy.devices.show') }))
    expect(within(block).getByText(hostile)).toBeTruthy()
    expect(block.querySelector('img')).toBeNull()
    expect(block.querySelector('b')).toBeNull()
  })

  it('says in words why the whole check could not start', async () => {
    vi.mocked(api.privacyCheck).mockRejectedValue(
      'Wait until the current band change, Refresh IP, APN change, auto-scan or speed test has finished.',
    )
    render(PrivacyCheck)
    await fireEvent.click(screen.getByRole('button', { name: t('privacy.run') }))
    await waitFor(() => expect(screen.getByRole('alert').textContent).toContain(t('pageScripts.refuse.busy')))
    expect((screen.getByRole('button', { name: t('privacy.run') }) as HTMLButtonElement).disabled).toBe(false)
  })

  it('says whether the router can be reached from the internet, from what the app already knows', () => {
    session('loggedIn', 'cgnat')
    render(PrivacyCheck)
    expect(within(screen.getByTestId('privacy-reach')).getByText(t('privacy.reach.shared'))).toBeTruthy()
    cleanup()
    session('loggedIn', 'public')
    render(PrivacyCheck)
    expect(within(screen.getByTestId('privacy-reach')).getByText(t('privacy.reach.public'))).toBeTruthy()
    cleanup()
    session('loggedIn', null)
    render(PrivacyCheck)
    expect(within(screen.getByTestId('privacy-reach')).getByText(t('privacy.reach.unknown'))).toBeTruthy()
    expect(api.privacyCheck).not.toHaveBeenCalled()
  })

  it('gives no assurance about IPv6, which the address type does not cover', () => {
    session('loggedIn', 'cgnat', false)
    render(PrivacyCheck)
    expect(within(screen.getByTestId('privacy-reach')).queryByText(t('privacy.reach.ipv6'))).toBeNull()
    cleanup()
    session('loggedIn', 'cgnat', true)
    render(PrivacyCheck)
    const reach = within(screen.getByTestId('privacy-reach'))
    expect(reach.getByText(t('privacy.reach.shared'))).toBeTruthy()
    expect(reach.getByText(t('privacy.reach.ipv6'))).toBeTruthy()
    // The shared-address sentence only speaks for IPv4.
    expect(t('privacy.reach.shared')).toContain('IPv4')
    expect(t('privacy.reach.public')).toContain('IPv4')
  })

  it('shows the DNS server masked until asked for in full', async () => {
    vi.mocked(api.dnsCheck).mockResolvedValue({ resolver: '203.0.113.5' })
    render(PrivacyCheck)
    const tile = within(screen.getByTestId('privacy-dns'))
    await fireEvent.click(tile.getByRole('button', { name: t('privacy.dns.check') }))
    await waitFor(() => expect(tile.getByTestId('privacy-dns-address').textContent).toBe('203.0.x.x'))
    expect(screen.queryByText('203.0.113.5')).toBeNull()
    await fireEvent.click(tile.getByRole('button', { name: t('network.wan.showFull') }))
    expect(tile.getByTestId('privacy-dns-address').textContent).toBe('203.0.113.5')
    // The router isn't read for this.
    expect(api.privacyCheck).not.toHaveBeenCalled()
  })

  it('works for the DNS check while logged out, since it does not use the router', async () => {
    session('loggedOut')
    vi.mocked(api.dnsCheck).mockResolvedValue({ resolver: null })
    render(PrivacyCheck)
    const tile = within(screen.getByTestId('privacy-dns'))
    await fireEvent.click(tile.getByRole('button', { name: t('privacy.dns.check') }))
    await waitFor(() => expect(tile.getByText(t('privacy.dns.none'))).toBeTruthy())
  })
})
