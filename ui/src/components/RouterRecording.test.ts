import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))
vi.mock('../lib/backend', () => ({
  api: {
    captureStart: vi.fn(),
    captureStatus: vi.fn(),
    captureStop: vi.fn(),
    capturePreview: vi.fn(),
    captureSave: vi.fn(),
    captureDiscard: vi.fn(),
    openRouterIssue: vi.fn(),
  },
  onCaptureStatus: vi.fn(),
}))

import { api, onCaptureStatus } from '../lib/backend'
import { setLanguage, t } from '../lib/i18n'
import { resetRecording } from '../lib/recording.svelte'
import { resetRouterProbe, routerProbe } from '../lib/routerProbe.svelte'
import RouterRecording from './RouterRecording.svelte'
import type { CapturePreview } from '../lib/bindings/CapturePreview'
import type { CaptureStatus } from '../lib/bindings/CaptureStatus'

const idle: CaptureStatus = { phase: 'idle', requests: 0, limit: null }
const running: CaptureStatus = { phase: 'recording', requests: 12, limit: null }
const stopped: CaptureStatus = { phase: 'stopped', requests: 12, limit: null }
const preview: CapturePreview = {
  text: '{"format":"router-band-tuner-capture/1"}',
  bytes: 2048,
  truncated: false,
  requests: 12,
  codeIncluded: false,
  limit: null,
  problems: [],
}

const button = (key: Parameters<typeof t>[0]) => screen.getByRole('button', { name: t(key) })
const noButton = (key: Parameters<typeof t>[0]) => screen.queryByRole('button', { name: t(key) })
/** The backend's own event, as the component would get it. */
let emit: (s: CaptureStatus) => void = () => {}

/** Renders with the backend already in `status`, and waits until the component has read it. */
async function mounted(status: CaptureStatus) {
  vi.mocked(api.captureStatus).mockResolvedValue(status)
  render(RouterRecording)
  await waitFor(() => expect(api.captureStatus).toHaveBeenCalled())
  await Promise.resolve()
}

/** Opens the "before you start" points and presses the button that opens the router's page. */
async function begin() {
  await fireEvent.click(button('recording.start'))
  await fireEvent.click(button('recording.begin'))
}

describe('RouterRecording', () => {
  beforeEach(() => {
    resetRecording()
    resetRouterProbe()
    setLanguage('en')
    vi.mocked(onCaptureStatus).mockImplementation(async (cb) => {
      emit = cb
      return () => {}
    })
    vi.mocked(api.captureStart).mockResolvedValue(null)
    vi.mocked(api.captureStop).mockResolvedValue(preview)
    vi.mocked(api.capturePreview).mockResolvedValue({ ...preview, codeIncluded: true, text: '{"with":"code"}' })
    vi.mocked(api.captureSave).mockResolvedValue(true)
    vi.mocked(api.captureDiscard).mockResolvedValue(undefined)
    vi.mocked(api.openRouterIssue).mockResolvedValue(undefined)
  })
  afterEach(() => {
    cleanup()
    vi.clearAllMocks()
    vi.useRealTimers()
  })

  it('says what will happen before anything opens, and can be backed out of', async () => {
    await mounted(idle)
    expect(screen.queryByText(t('recording.before2'))).toBeNull()
    await fireEvent.click(button('recording.start'))
    // The three things a person must know first: what to do, that it is real, that nothing is sent.
    for (const key of ['recording.before1', 'recording.before2', 'recording.before3', 'recording.passwordSafe'] as const) {
      expect(screen.getByText(t(key))).toBeTruthy()
    }
    expect(api.captureStart).not.toHaveBeenCalled()
    await fireEvent.click(button('recording.cancel'))
    expect(screen.queryByText(t('recording.before2'))).toBeNull()
    expect(api.captureStart).not.toHaveBeenCalled()
  })

  it('opens the router page with a title in the user language, then shows the count and Stop', async () => {
    await mounted(idle)
    vi.mocked(api.captureStatus).mockResolvedValue(running)
    await begin()
    await waitFor(() => expect(api.captureStart).toHaveBeenCalledWith(t('recording.windowTitle')))
    await waitFor(() => expect(screen.getByText(t('recording.running', { n: '12' }))).toBeTruthy())
    expect(button('recording.stop')).toBeTruthy()
    expect(noButton('recording.start')).toBeNull()
  })

  it('says why when a recording cannot start, and opens nothing', async () => {
    await mounted(idle)
    await fireEvent.click(button('recording.start'))
    // The points stay open after a refusal, so the same button can be pressed again.
    for (const why of ['loggedIn', 'busy', 'demo', 'alreadyOpen', 'badAddress'] as const) {
      vi.mocked(api.captureStart).mockResolvedValue(why)
      await fireEvent.click(button('recording.begin'))
      await waitFor(() => expect(screen.getByRole('alert').textContent).toContain(t(`recording.refused.${why}`)))
      expect(noButton('recording.stop')).toBeNull()
    }
  })

  it('keeps counting while the recording runs', async () => {
    vi.useFakeTimers()
    vi.mocked(api.captureStatus).mockResolvedValue(running)
    render(RouterRecording)
    await vi.advanceTimersByTimeAsync(10)
    vi.mocked(api.captureStatus).mockResolvedValue({ ...running, requests: 40 })
    await vi.advanceTimersByTimeAsync(1100)
    expect(screen.getByText(t('recording.running', { n: '40' }))).toBeTruthy()
  })

  it('says which limit stopped it taking more', async () => {
    await mounted({ ...running, limit: 'time' })
    expect(screen.getByText(t('recording.limit.time'))).toBeTruthy()
  })

  it('shows exactly what would be saved, as text, before Save can be pressed', async () => {
    await mounted(running)
    vi.mocked(api.captureStatus).mockResolvedValue(stopped)
    await fireEvent.click(button('recording.stop'))
    const box = (await screen.findByRole('textbox')) as HTMLTextAreaElement
    expect(box.readOnly).toBe(true)
    expect(box.value).toBe(preview.text)
    expect(screen.getByText(t('recording.reviewBody'))).toBeTruthy()
    expect(screen.getByText(t('recording.requests', { n: '12' }))).toBeTruthy()
    expect(screen.getByText('2 KB')).toBeTruthy()
    expect(button('recording.save')).toBeTruthy()
    expect(screen.queryByText(t('recording.truncated'))).toBeNull()
  })

  it('says when only the start of a large file is shown', async () => {
    vi.mocked(api.captureStop).mockResolvedValue({ ...preview, truncated: true, bytes: 5 * 1024 * 1024 })
    await mounted(running)
    vi.mocked(api.captureStatus).mockResolvedValue(stopped)
    await fireEvent.click(button('recording.stop'))
    expect(await screen.findByText(t('recording.truncated'))).toBeTruthy()
    expect(screen.getByText('5 MB')).toBeTruthy()
  })

  it('rebuilds the file when the router code is asked for, and shows the new text', async () => {
    await mounted(running)
    vi.mocked(api.captureStatus).mockResolvedValue(stopped)
    await fireEvent.click(button('recording.stop'))
    await fireEvent.click(await screen.findByRole('checkbox', { name: t('recording.includeCode') }))
    await waitFor(() => expect(api.capturePreview).toHaveBeenCalledWith(true))
    await waitFor(() => expect((screen.getByRole('textbox') as HTMLTextAreaElement).value).toBe('{"with":"code"}'))
  })

  it('saves, forgets the recording, and offers the New router form for a router the app does not support', async () => {
    routerProbe.view = { support: 'unknown', vendor: 'ZTE', model: null, hardware: null, firmware: null }
    await mounted(running)
    vi.mocked(api.captureStatus).mockResolvedValue(stopped)
    await fireEvent.click(button('recording.stop'))
    await screen.findByRole('textbox')
    vi.mocked(api.captureStatus).mockResolvedValue(idle)
    await fireEvent.click(button('recording.save'))
    await waitFor(() => expect(api.captureSave).toHaveBeenCalled())
    await waitFor(() => expect(screen.queryByRole('textbox')).toBeNull())
    await fireEvent.click(button('unsupported.openIssue'))
    expect(api.openRouterIssue).toHaveBeenCalled()
  })

  it('only says it is saved on a router the app supports: the form is for new routers', async () => {
    await mounted(running)
    vi.mocked(api.captureStatus).mockResolvedValue(stopped)
    await fireEvent.click(button('recording.stop'))
    await screen.findByRole('textbox')
    vi.mocked(api.captureStatus).mockResolvedValue(idle)
    await fireEvent.click(button('recording.save'))
    await waitFor(() => expect(screen.getAllByText(t('recording.saved')).length).toBeGreaterThan(0))
    expect(noButton('unsupported.openIssue')).toBeNull()
  })

  it('keeps the review open when the save dialog is closed', async () => {
    vi.mocked(api.captureSave).mockResolvedValue(false)
    await mounted(running)
    vi.mocked(api.captureStatus).mockResolvedValue(stopped)
    await fireEvent.click(button('recording.stop'))
    await screen.findByRole('textbox')
    await fireEvent.click(button('recording.save'))
    await waitFor(() => expect(api.captureSave).toHaveBeenCalled())
    expect(screen.getByRole('textbox')).toBeTruthy()
    expect(noButton('unsupported.openIssue')).toBeNull()
  })

  it('cannot save a file the privacy check stopped, and says what it found', async () => {
    vi.mocked(api.captureStop).mockResolvedValue({ ...preview, text: '', bytes: 0, problems: ['an IPv4 address'] })
    await mounted(running)
    vi.mocked(api.captureStatus).mockResolvedValue(stopped)
    await fireEvent.click(button('recording.stop'))
    await waitFor(() =>
      expect(screen.getByRole('alert').textContent).toContain(t('recording.blocked', { what: 'an IPv4 address' })),
    )
    expect(noButton('recording.save')).toBeNull()
    expect(button('recording.discard')).toBeTruthy()
  })

  it('throws the recording away on Discard, while recording or at the review', async () => {
    await mounted(running)
    vi.mocked(api.captureStatus).mockResolvedValue(idle)
    await fireEvent.click(button('recording.discard'))
    await waitFor(() => expect(api.captureDiscard).toHaveBeenCalledTimes(1))
    await waitFor(() => expect(button('recording.start')).toBeTruthy())
    // Discarding is the user's own doing: no "the window was closed" note.
    expect(screen.queryByText(t('recording.closed'))).toBeNull()
  })

  it('says the recording was thrown away when its window is closed by hand', async () => {
    await mounted(running)
    emit(idle)
    await waitFor(() => expect(screen.getByText(t('recording.closed'))).toBeTruthy())
    expect(button('recording.start')).toBeTruthy()
  })

  it('picks up a recording that is waiting for its review', async () => {
    vi.mocked(api.capturePreview).mockResolvedValue(preview)
    await mounted(stopped)
    await waitFor(() => expect(api.capturePreview).toHaveBeenCalledWith(false))
    expect(((await screen.findByRole('textbox')) as HTMLTextAreaElement).value).toBe(preview.text)
  })

  it('reads in Arabic too', async () => {
    setLanguage('ar')
    await mounted(idle)
    expect(button('recording.start').textContent).toContain('سجّل')
  })
})
