import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/svelte'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))

import { invoke } from '@tauri-apps/api/core'
import { app } from '../lib/store.svelte'
import { setLanguage, t } from '../lib/i18n'
import { settingsFixture, snap } from '../test-fixtures'
import type { ApnProfile } from '../lib/bindings/ApnProfile'
import type { ApnProfiles as Profiles } from '../lib/bindings/ApnProfiles'
import ApnProfiles from './ApnProfiles.svelte'

function profile(o: Partial<ApnProfile>): ApnProfile {
  return {
    index: 1,
    name: 'Operator internet',
    apn: 'internet.example',
    apnStatic: true,
    username: '',
    hasPassword: false,
    auth: 'auto',
    ipType: 'ipv4v6',
    readOnly: false,
    valid: true,
    fingerprint: '0123456789abcdef',
    ...o,
  }
}

const LIST: Profiles = {
  current: 1,
  autoApn: false,
  profiles: [
    profile({ index: 1, readOnly: true }),
    profile({ index: 2, name: 'Network default', apn: null, apnStatic: false, ipType: 'ipv4' }),
    profile({ index: 3, name: 'Old', valid: false }),
  ],
}

describe('ApnProfiles', () => {
  afterEach(() => {
    cleanup()
    app.snapshot = null
    vi.mocked(invoke).mockReset()
  })

  it('lists the profiles in words: in use, from the operator, set by the network', () => {
    app.settings = settingsFixture()
    app.snapshot = snap({ apn: LIST })
    render(ApnProfiles)
    const rows = screen.getAllByTestId('apn-row')
    expect(rows).toHaveLength(3)
    // "In use" is a word with an icon, not a colour alone.
    expect(rows[0].querySelector('[data-testid="apn-in-use"]')?.textContent?.trim()).toBe(t('network.apn.inUse'))
    expect(rows[0].querySelector('[data-testid="apn-operator"]')?.textContent?.trim()).toBe(t('network.apn.operator'))
    expect(rows[0].querySelector('[data-testid="apn-operator"] svg')).not.toBeNull()
    expect(rows[0].textContent).toContain('internet.example')
    expect(rows[0].textContent).toContain(t('network.apn.ipType.ipv4v6'))
    expect(rows[1].querySelector('[data-testid="apn-in-use"]')).toBeNull()
    expect(rows[1].querySelector('[data-testid="apn-value"]')?.textContent).toBe(t('network.apn.byNetwork'))
    expect(rows[1].textContent).toContain(t('network.apn.ipType.ipv4'))
    expect(rows[2].querySelector('[data-testid="apn-invalid"]')).not.toBeNull()
    expect(screen.getByText(t('network.apn.explainer'))).toBeTruthy()
    // The fixture's driver hasn't confirmed the calls on its model.
    expect(screen.getByTestId('apn-unconfirmed').textContent).toBe(t('network.apn.unconfirmed'))
    expect(screen.queryByTestId('apn-auto')).toBeNull()
  })

  it('explains Auto APN, and drops the unconfirmed note once confirmed', () => {
    app.settings = settingsFixture()
    const s = snap({ apn: { ...LIST, autoApn: true } })
    s.capabilities = { ...s.capabilities!, apn: { read: true, switch: false, edit: false, confirmed: true } }
    app.snapshot = s
    render(ApnProfiles)
    expect(screen.getByTestId('apn-auto').textContent).toBe(t('network.apn.autoOn'))
    expect(screen.queryByTestId('apn-unconfirmed')).toBeNull()
  })

  it('says why there is no list, and hides the card on a router without profiles', () => {
    app.settings = settingsFixture()
    app.snapshot = snap()
    render(ApnProfiles)
    expect(screen.getByTestId('apn-unknown').textContent).toBe(t('network.apn.unknown'))
    cleanup()
    app.snapshot = snap({ session: { kind: 'loggedOut' }, apn: LIST })
    render(ApnProfiles)
    expect(screen.getByTestId('apn-unknown').textContent).toBe(t('network.apn.loggedOut'))
    expect(document.body.textContent).not.toContain('internet.example')
    cleanup()
    const none = snap({ apn: LIST })
    none.capabilities = { ...none.capabilities!, apn: { read: false, switch: false, edit: false, confirmed: false } }
    app.snapshot = none
    render(ApnProfiles)
    expect(screen.queryByTestId('apn-card')).toBeNull()
  })

  it('reads the list again on request', async () => {
    app.settings = settingsFixture()
    app.snapshot = snap({ apn: LIST })
    vi.mocked(invoke).mockResolvedValue(undefined)
    render(ApnProfiles)
    await fireEvent.click(screen.getByTestId('apn-refresh'))
    expect(invoke).toHaveBeenCalledWith('apn_refresh')
  })

  it('offers "Use this APN" on the profiles that can be chosen, and switches only after the dialog', async () => {
    app.settings = settingsFixture()
    const s = snap({ apn: LIST })
    s.capabilities = { ...s.capabilities!, apn: { read: true, switch: true, edit: false, confirmed: false } }
    app.snapshot = s
    vi.mocked(invoke).mockImplementation(async (cmd: string) =>
      cmd === 'apn_preview' ? { fields: [['SetDefault', '2']], body: '<request><SetDefault>2</SetDefault></request>' } : undefined,
    )
    render(ApnProfiles)
    const rows = screen.getAllByTestId('apn-row')
    // Not on the profile in use, nor on an incomplete one.
    expect(rows[0].querySelector('[data-testid="apn-use"]')).toBeNull()
    expect(rows[2].querySelector('[data-testid="apn-use"]')).toBeNull()
    const use = rows[1].querySelector('[data-testid="apn-use"]') as HTMLButtonElement
    expect(use.disabled).toBe(false)
    await fireEvent.click(use)
    expect(screen.getByRole('dialog')).toBeTruthy()
    expect(invoke).toHaveBeenCalledWith('apn_preview', { index: 2, fingerprint: '0123456789abcdef' })
    expect(invoke).not.toHaveBeenCalledWith('apn_switch', expect.anything())
    expect(screen.getByRole('dialog').textContent).toContain('Network default')
    await fireEvent.click(screen.getByTestId('apn-sw-start'))
    expect(invoke).toHaveBeenCalledWith('apn_switch', { index: 2, fingerprint: '0123456789abcdef' })
    await vi.waitFor(() => expect(screen.queryByRole('dialog')).toBeNull())
  })

  it('keeps the dialog open with the reason when the switch is refused', async () => {
    app.settings = settingsFixture()
    const s = snap({ apn: LIST })
    s.capabilities = { ...s.capabilities!, apn: { read: true, switch: true, edit: false, confirmed: false } }
    app.snapshot = s
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'apn_switch') throw { kind: 'validation', message: 'that APN profile changed on the router' }
      return { fields: [], body: '' }
    })
    render(ApnProfiles)
    await fireEvent.click(screen.getAllByTestId('apn-use')[0])
    await fireEvent.click(screen.getByTestId('apn-sw-start'))
    await vi.waitFor(() => expect(screen.getByTestId('apn-sw-error').textContent).toBe(t('network.apn.refuse.changed')))
    expect(screen.getByRole('dialog')).toBeTruthy()
  })

  it('says why no APN can be chosen while something else holds the router', () => {
    app.settings = settingsFixture()
    const s = snap({
      apn: LIST,
      safeApply: { phase: { kind: 'probation' }, secondsLeft: 10, previous: null, requested: null },
    })
    s.capabilities = { ...s.capabilities!, apn: { read: true, switch: true, edit: false, confirmed: false } }
    app.snapshot = s
    render(ApnProfiles)
    expect(screen.getByTestId('apn-why').textContent).toBe(t('network.apn.why.bandChange'))
    expect((screen.getAllByTestId('apn-use')[0] as HTMLButtonElement).disabled).toBe(true)
    cleanup()
    // Auto APN on: the note explains it, and nothing can be chosen.
    const auto = snap({ apn: { ...LIST, autoApn: true } })
    auto.capabilities = s.capabilities
    app.snapshot = auto
    render(ApnProfiles)
    expect(screen.getByTestId('apn-auto')).toBeTruthy()
    expect((screen.getAllByTestId('apn-use')[0] as HTMLButtonElement).disabled).toBe(true)
  })

  it('has text in Arabic too', () => {
    setLanguage('ar')
    try {
      for (const k of ['title', 'explainer', 'byNetwork', 'operator', 'inUse', 'autoOn', 'unconfirmed', 'use'] as const) {
        expect(t(`network.apn.${k}`)).not.toBe(`network.apn.${k}`)
      }
      for (const k of ['title', 'body', 'step1', 'step2', 'step3', 'start'] as const) {
        expect(t(`network.apn.switch.${k}`)).not.toBe(`network.apn.switch.${k}`)
      }
    } finally {
      setLanguage('en')
    }
  })
})
