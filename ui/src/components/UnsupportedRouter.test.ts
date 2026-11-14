import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))
vi.mock('../lib/backend', () => ({
  api: {
    routerProbeStatus: vi.fn(),
    prepareRouterReport: vi.fn(),
    saveDiagnostics: vi.fn(),
    openRouterIssue: vi.fn(),
  },
  onRouterProbe: vi.fn(),
}))

import { api, onRouterProbe } from '../lib/backend'
import { t } from '../lib/i18n'
import { resetRouterProbe } from '../lib/routerProbe.svelte'
import UnsupportedRouter from './UnsupportedRouter.svelte'
import type { ProbeView } from '../lib/bindings/ProbeView'

const unknown: ProbeView = { support: 'unknown', vendor: 'TP-Link', model: 'TL-MR6400', hardware: 'MR6400 v5.0', firmware: '1.2.0' }
const title = () => screen.queryByRole('heading', { name: t('unsupported.title') })

describe('UnsupportedRouter', () => {
  beforeEach(() => {
    resetRouterProbe()
    vi.mocked(onRouterProbe).mockResolvedValue(() => {})
    vi.mocked(api.routerProbeStatus).mockResolvedValue(unknown)
    vi.mocked(api.prepareRouterReport).mockResolvedValue('{"format":"router-band-tuner-unknown-router/1"}')
    vi.mocked(api.saveDiagnostics).mockResolvedValue(true)
    vi.mocked(api.openRouterIssue).mockResolvedValue(undefined)
  })
  afterEach(() => {
    cleanup()
    vi.clearAllMocks()
  })

  it('stays away from a supported or unreachable router, or before it has looked', async () => {
    for (const v of [null, { ...unknown, support: 'supported' }, { ...unknown, support: 'unreachable' }] as (ProbeView | null)[]) {
      resetRouterProbe()
      vi.mocked(api.routerProbeStatus).mockResolvedValue(v)
      render(UnsupportedRouter)
      await waitFor(() => expect(api.routerProbeStatus).toHaveBeenCalled())
      await Promise.resolve()
      expect(title()).toBeNull()
      cleanup()
      vi.mocked(api.routerProbeStatus).mockClear()
    }
  })

  it('names what it found and says no password is needed', async () => {
    const view = render(UnsupportedRouter)
    await waitFor(() => expect(title()).toBeTruthy())
    expect(view.container.textContent).toContain('TP-Link TL-MR6400')
    expect(view.container.textContent).toContain('MR6400 v5.0')
    expect(view.container.textContent).toContain(t('unsupported.noPassword'))
  })

  it('still offers the report when it could not read a model', async () => {
    vi.mocked(api.routerProbeStatus).mockResolvedValue({ support: 'unknown', vendor: null, model: null, hardware: null, firmware: null })
    const view = render(UnsupportedRouter)
    await waitFor(() => expect(title()).toBeTruthy())
    expect(view.container.textContent).toContain(t('unsupported.foundNothing'))
    expect(screen.getByRole('button', { name: t('unsupported.help') })).toBeTruthy()
  })

  it('shows the whole file before it can be saved, then guides to the issue', async () => {
    render(UnsupportedRouter)
    await waitFor(() => expect(title()).toBeTruthy())
    expect(screen.queryByRole('button', { name: t('diagnostics.save') })).toBeNull()
    expect(api.prepareRouterReport).not.toHaveBeenCalled()

    await fireEvent.click(screen.getByRole('button', { name: t('unsupported.help') }))
    const box = (await screen.findByRole('textbox')) as HTMLTextAreaElement
    expect(box.value).toContain('router-band-tuner-unknown-router/1')
    expect(api.saveDiagnostics).not.toHaveBeenCalled()

    await fireEvent.click(screen.getByRole('button', { name: t('diagnostics.save') }))
    await waitFor(() => expect(api.saveDiagnostics).toHaveBeenCalledOnce())
    await fireEvent.click(screen.getByRole('button', { name: t('unsupported.openIssue') }))
    expect(api.openRouterIssue).toHaveBeenCalledOnce()
  })

  it('shows a privacy stop, and can be dismissed', async () => {
    vi.mocked(api.prepareRouterReport).mockRejectedValue('Export stopped: the privacy check found a MAC address. Nothing was saved.')
    render(UnsupportedRouter)
    await waitFor(() => expect(title()).toBeTruthy())
    await fireEvent.click(screen.getByRole('button', { name: t('unsupported.help') }))
    await waitFor(() => expect(screen.getByRole('alert').textContent).toContain('privacy check'))
    await fireEvent.click(screen.getByRole('button', { name: t('app.dismiss') }))
    expect(title()).toBeNull()
  })
})
