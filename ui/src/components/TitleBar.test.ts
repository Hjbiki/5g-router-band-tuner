import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte'

const win = {
  minimize: vi.fn(() => Promise.resolve()),
  toggleMaximize: vi.fn(() => Promise.resolve()),
  close: vi.fn(() => Promise.resolve()),
  destroy: vi.fn(() => Promise.resolve()),
  isMaximized: vi.fn(() => Promise.resolve(false)),
  onResized: vi.fn(() => Promise.resolve(() => {})),
}
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => win }))
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))

import { t } from '../lib/i18n'
import TitleBar from './TitleBar.svelte'

describe('TitleBar', () => {
  beforeEach(() => win.isMaximized.mockResolvedValue(false))
  afterEach(() => {
    cleanup()
    vi.clearAllMocks()
  })

  it('is a drag region with the app name', () => {
    render(TitleBar)
    const bar = screen.getByRole('banner', { name: t('shell.titleBar') })
    expect(bar.hasAttribute('data-tauri-drag-region')).toBe(true)
    expect(bar.textContent).toContain(t('app.title'))
  })

  it('the app mark lets presses through to the drag region under it', () => {
    const { container } = render(TitleBar)
    const mark = container.querySelector('svg.mark')!
    expect(mark.getAttribute('pointer-events')).toBe('none')
    expect(mark.parentElement?.hasAttribute('data-tauri-drag-region')).toBe(true)
  })

  it('close asks the window to close (Rust hides it to the tray), never destroys it', async () => {
    render(TitleBar)
    await fireEvent.click(screen.getByRole('button', { name: t('shell.close') }))
    expect(win.close).toHaveBeenCalledOnce()
    expect(win.destroy).not.toHaveBeenCalled()
  })

  it('minimises and toggles maximise, naming the button by the current state', async () => {
    win.isMaximized.mockResolvedValue(true)
    render(TitleBar)
    await fireEvent.click(screen.getByRole('button', { name: t('shell.minimise') }))
    expect(win.minimize).toHaveBeenCalledOnce()
    const restore = await waitFor(() => screen.getByRole('button', { name: t('shell.restore') }))
    await fireEvent.click(restore)
    expect(win.toggleMaximize).toHaveBeenCalledOnce()
    expect(win.onResized).toHaveBeenCalledOnce()
  })
})
