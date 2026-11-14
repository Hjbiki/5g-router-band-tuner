import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, render, screen } from '@testing-library/svelte'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))

import { app } from '../lib/store.svelte'
import { t } from '../lib/i18n'
import { settingsFixture } from '../test-fixtures'
import { resultFixture } from '../lib/speedtest.fixtures'
import SpeedHistoryTable from './SpeedHistoryTable.svelte'

describe('SpeedHistoryTable', () => {
  afterEach(() => cleanup())

  it('shows each test\'s IP range, or Unknown for older tests', () => {
    app.settings = settingsFixture()
    render(SpeedHistoryTable, {
      rows: [resultFixture({ atMs: 2_000, ipRange: '10.45' }), resultFixture({ atMs: 1_000 })],
    })
    expect(screen.getByRole('columnheader', { name: new RegExp(t('speedtest.history.ipRange')) })).toBeTruthy()
    const cells = screen.getAllByTestId('ip-range').map((c) => c.textContent?.trim())
    expect(cells).toEqual(['10.45.x.x', t('app.unknown')])
  })

  it('says when the internet address changed during a test', () => {
    app.settings = settingsFixture()
    render(SpeedHistoryTable, {
      rows: [
        resultFixture({ atMs: 2_000, ipRange: '10.45', invalidReason: 'ipChanged' }),
        resultFixture({ atMs: 1_000, invalidReason: 'bandChange' }),
      ],
    })
    const notes = screen.getAllByTestId('not-comparable').map((c) => c.textContent)
    expect(notes).toEqual([t('speedtest.history.notComparableIp'), t('speedtest.history.notComparable')])
  })
})
