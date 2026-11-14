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

describe('SpeedHistoryTable, a test that lost its upload', () => {
  afterEach(() => cleanup())

  it('keeps its numbers on screen and says the upload was not measured', () => {
    app.settings = settingsFixture()
    render(SpeedHistoryTable, {
      rows: [resultFixture({ atMs: 2_000, downloadMbps: 185, uploadMbps: null, invalidReason: 'incomplete' })],
    })
    // Not the "not comparable" row, which hides the numbers.
    expect(screen.queryByTestId('not-comparable')).toBeNull()
    expect(screen.getByTestId('upload-missing').textContent?.trim()).toBe(t('speedtest.history.uploadMissing'))
    expect(screen.getByText(/^185/)).toBeTruthy()
  })
})
