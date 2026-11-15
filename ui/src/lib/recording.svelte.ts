import type { CapturePreview } from './bindings/CapturePreview'
import type { CaptureStatus } from './bindings/CaptureStatus'
import { api, onCaptureStatus } from './backend'
import { app } from './store.svelte'

const IDLE: CaptureStatus = { phase: 'idle', requests: 0, limit: null, enough: false }

/**
 * A recording of the router's own page: where it stands, and the cleaned file once it is stopped.
 * Kept here so both places that offer it (the unsupported-router notice and About) show one state.
 */
export const recording: {
  status: CaptureStatus
  /** The cleaned file's start and size, exactly as it would be saved. */
  preview: CapturePreview | null
  /** How many places on screen show the recording right now (the notice, About). */
  places: number
} = $state({ status: IDLE, preview: null, places: 0 })

/**
 * A status from the backend. A recording can end from its own window (its Stop item, or its X),
 * and then its review must be in front of the user: when no page is showing it, About opens.
 */
export function takeCaptureStatus(next: CaptureStatus): void {
  const ended = recording.status.phase === 'recording' && next.phase === 'stopped'
  if (ended && recording.places === 0) app.tab = 'about'
  if (next.phase !== 'stopped') recording.preview = null
  recording.status = next
}

let loading: Promise<CapturePreview> | null = null

/**
 * The cleaned file of a recording that is waiting for its review, without the router's scripts.
 * Every place that shows the recording asks as soon as it stops, and cleaning a large one takes
 * seconds: they share one request.
 */
export function loadPreview(): Promise<CapturePreview> {
  loading ??= api.capturePreview(false).finally(() => {
    loading = null
  })
  return loading
}

let started: Promise<void> | null = null

export function ensureRecording(): Promise<void> {
  started ??= (async () => {
    await onCaptureStatus((s) => takeCaptureStatus(s))
    takeCaptureStatus(await api.captureStatus())
  })().catch(() => {
    started = null
  })
  return started
}

/** For tests. */
export function resetRecording(): void {
  started = null
  loading = null
  recording.status = IDLE
  recording.preview = null
  recording.places = 0
}
