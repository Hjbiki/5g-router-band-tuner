import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))
vi.mock('../lib/backend', () => ({
  api: { prepareDiagnostics: vi.fn(), saveDiagnostics: vi.fn() },
}))

import { api } from '../lib/backend'
import { t } from '../lib/i18n'
import DiagnosticsExport from './DiagnosticsExport.svelte'

describe('DiagnosticsExport', () => {
  afterEach(() => {
    cleanup()
    vi.clearAllMocks()
  })

  it('shows the redacted file before it can be saved', async () => {
    vi.mocked(api.prepareDiagnostics).mockResolvedValue('{"format":"router-band-tuner-diagnostics/1"}')
    vi.mocked(api.saveDiagnostics).mockResolvedValue(true)
    render(DiagnosticsExport)
    expect(screen.queryByRole('button', { name: t('diagnostics.save') })).toBeNull()

    await fireEvent.click(screen.getByRole('button', { name: t('diagnostics.export') }))
    await waitFor(() => expect((screen.getByRole('textbox') as HTMLTextAreaElement).value).toContain('diagnostics/1'))
    expect(api.saveDiagnostics).not.toHaveBeenCalled()

    await fireEvent.click(screen.getByRole('button', { name: t('diagnostics.save') }))
    await waitFor(() => expect(api.saveDiagnostics).toHaveBeenCalledOnce())
    await waitFor(() => expect(screen.queryByRole('textbox')).toBeNull())
  })

  it('reports a blocked export and offers no save', async () => {
    vi.mocked(api.prepareDiagnostics).mockRejectedValue('Export stopped: the privacy check found a MAC address.')
    render(DiagnosticsExport)
    await fireEvent.click(screen.getByRole('button', { name: t('diagnostics.export') }))
    await waitFor(() => expect(screen.getByRole('alert').textContent).toContain('privacy check'))
    expect(screen.queryByRole('button', { name: t('diagnostics.save') })).toBeNull()
  })
})
