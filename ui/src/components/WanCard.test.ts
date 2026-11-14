import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/svelte'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))

import { invoke } from '@tauri-apps/api/core'
import { app } from '../lib/store.svelte'
import { t } from '../lib/i18n'
import { settingsFixture, snap } from '../test-fixtures'
import WanCard from './WanCard.svelte'
import { closeIpRefresh, ipRefreshUi } from '../lib/ipRefresh.svelte'

const WAN = { ipv4: '10.45.37.142', range: '10.45', kind: 'private', ipv6: false } as const

describe('WanCard', () => {
  afterEach(() => {
    cleanup()
    app.snapshot = null
  })

  it('shows the range, and the full address only on request', async () => {
    app.settings = settingsFixture()
    app.snapshot = snap({ wan: WAN })
    render(WanCard)
    const address = () => screen.getByTestId('wan-address').textContent
    expect(address()).toBe('10.45.x.x')
    expect(document.body.textContent).not.toContain('10.45.37.142')
    const toggle = screen.getByRole('button', { name: t('network.wan.showFull') })
    expect(toggle.getAttribute('aria-pressed')).toBe('false')
    await fireEvent.click(toggle)
    expect(address()).toBe('10.45.37.142')
    await fireEvent.click(screen.getByRole('button', { name: t('network.wan.hideFull') }))
    expect(address()).toBe('10.45.x.x')
  })

  it('names the address type in words with an icon, and says whether there is IPv6', () => {
    app.settings = settingsFixture()
    app.snapshot = snap({ wan: { ipv4: '100.70.3.4', range: '100.70', kind: 'cgnat', ipv6: true } })
    render(WanCard)
    const kind = screen.getByTestId('wan-kind')
    expect(kind.textContent?.trim()).toBe(t('network.kind.cgnat'))
    expect(kind.querySelector('svg')).not.toBeNull()
    expect(screen.getByTestId('wan-ipv6').textContent).toBe(t('network.wan.yes'))
  })

  it('says when there is no address, and hides it while logged out', () => {
    app.settings = settingsFixture()
    app.snapshot = snap({ wan: { ipv4: null, range: null, kind: null, ipv6: false } })
    render(WanCard)
    expect(screen.getByTestId('wan-address').textContent).toBe(t('network.wan.none'))
    expect(screen.queryByRole('button', { name: t('network.wan.showFull') })).toBeNull()
    cleanup()
    app.snapshot = snap({ session: { kind: 'loggedOut' }, wan: WAN })
    render(WanCard)
    expect(screen.getByTestId('wan-unknown').textContent).toBe(t('network.wan.loggedOut'))
    expect(document.body.textContent).not.toContain('10.45')
  })

  it('has text in Arabic too', async () => {
    const { setLanguage } = await import('../lib/i18n')
    setLanguage('ar')
    try {
      for (const k of ['cgnat', 'private', 'public'] as const) expect(t(`network.kind.${k}`)).not.toBe(`network.kind.${k}`)
    } finally {
      setLanguage('en')
    }
  })

  it('offers Refresh IP only with the mobile data switch, and only opens the dialog', async () => {
    app.settings = settingsFixture()
    app.snapshot = snap({ wan: WAN })
    const view = render(WanCard)
    expect(screen.queryByTestId('wan-refresh')).toBeNull()
    view.unmount()
    const s = snap({ wan: WAN })
    if (s.capabilities) s.capabilities = { ...s.capabilities, dataSwitch: true, ipMethods: ['dataSwitch'] }
    app.snapshot = s
    render(WanCard)
    await fireEvent.click(screen.getByTestId('wan-refresh'))
    expect(ipRefreshUi.open).toBe(true)
    expect(ipRefreshUi.prefill).toBeNull()
    expect(screen.getByText(t('ipRefresh.openHint'))).toBeTruthy()
    closeIpRefresh()
  })

  it('offers Refresh IP from the network search and words its hint for it', () => {
    app.settings = settingsFixture()
    const s = snap({ wan: WAN })
    if (s.capabilities) s.capabilities = { ...s.capabilities, dataSwitch: true, ipMethods: ['networkSearch'] }
    app.snapshot = s
    render(WanCard)
    expect(screen.getByTestId('wan-refresh')).toBeTruthy()
    expect(screen.getByText(t('ipRefresh.openHintSearch'))).toBeTruthy()
  })

  it('shows how the last band switch for a new IP ended, with its ranges, and offers the bands back', async () => {
    app.settings = settingsFixture()
    const restore = { networkMode: { kind: 'auto' }, lteBands: '4', legacyBandRaw: '0', nrOption: 'saNsa' } as const
    app.snapshot = snap({
      wan: WAN,
      ipReattach: { active: false, fromRange: null, last: { outcome: 'changed', from: '10.45', to: '10.62', restore } },
    })
    render(WanCard)
    const line = screen.getByTestId('wan-reattach-last').textContent ?? ''
    expect(line).toContain(t('ipReattach.lastLine', { outcome: t('ipReattach.outcome.changed') }))
    expect(line).toContain('10.45.x.x')
    expect(line).toContain('10.62.x.x')
    await fireEvent.click(screen.getByTestId('wan-reattach-restore'))
    expect(invoke).toHaveBeenCalledWith('apply_config', { cfg: restore })
    cleanup()
    // While it runs, its steps are in the safe-apply banner instead.
    app.snapshot = snap({ wan: WAN, ipReattach: { active: true, fromRange: '10.45', last: null } })
    render(WanCard)
    expect(screen.queryByTestId('wan-reattach-last')).toBeNull()
  })
})
