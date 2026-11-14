import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/svelte'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))

import { app } from '../lib/store.svelte'
import { t } from '../lib/i18n'
import { settingsFixture, snap, SUPPORTED_HEX } from '../test-fixtures'
import ConnectionCard from './ConnectionCard.svelte'

describe('ConnectionCard', () => {
  afterEach(() => {
    cleanup()
    app.snapshot = null
  })

  it('lists the allowed bands', () => {
    app.settings = settingsFixture()
    app.snapshot = snap()
    render(ConnectionCard)
    expect(screen.getByText(t('dashboard.allowedBands'))).toBeTruthy()
    expect(screen.getByTestId('allowed-bands').textContent).toBe('B1 · B3')
  })

  it('says "all" when nothing is locked out', () => {
    app.settings = settingsFixture()
    app.snapshot = snap({ config: { networkMode: { kind: 'auto' }, lteBands: SUPPORTED_HEX, legacyBandRaw: '0', nrOption: 'saNsa' } })
    render(ConnectionCard)
    expect(screen.getByText(t('dashboard.bandsAll'))).toBeTruthy()
    expect(screen.queryByTestId('allowed-bands')).toBeNull()
  })

  it('shows the WAN range and type in one line that opens Router', async () => {
    app.settings = settingsFixture()
    app.tab = 'dashboard'
    app.snapshot = snap({ wan: { ipv4: '10.45.37.142', range: '10.45', kind: 'private', ipv6: false } })
    render(ConnectionCard)
    const fact = screen.getByTestId('wan-fact')
    expect(fact.textContent).toContain('10.45.x.x')
    expect(fact.textContent).toContain(t('network.kindShort.private'))
    expect(fact.textContent).not.toContain('10.45.37.142')
    await fireEvent.click(fact)
    expect(app.tab).toBe('router')
  })

  it('points to Network while the address is unknown', () => {
    app.settings = settingsFixture()
    app.snapshot = snap()
    render(ConnectionCard)
    expect(screen.getByTestId('wan-fact').textContent?.trim()).toBe(t('network.wan.see'))
  })
})
