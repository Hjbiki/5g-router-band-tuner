import type { CapturePreview } from './bindings/CapturePreview'
import type { CaptureStatus } from './bindings/CaptureStatus'
import { api, onCaptureStatus } from './backend'

const IDLE: CaptureStatus = { phase: 'idle', requests: 0, limit: null }

/**
 * A recording of the router's own page: where it stands, and the cleaned file once it is stopped.
 * Kept here so both places that offer it (the unsupported-router notice and About) show one state.
 */
export const recording: {
  status: CaptureStatus
  /** The cleaned file's start and size, exactly as it would be saved. */
  preview: CapturePreview | null
  /** The window was closed while recording, which throws the recording away. */
  closedByHand: boolean
} = $state({ status: IDLE, preview: null, closedByHand: false })

/**
 * A status from the backend. A recording that ends without Stop or Discard was closed by hand;
 * `byUser` is true when this window asked for the change itself.
 */
export function takeCaptureStatus(next: CaptureStatus, byUser = false): void {
  if (recording.status.phase === 'recording' && next.phase === 'idle' && !byUser) recording.closedByHand = true
  if (next.phase !== 'idle') recording.closedByHand = false
  if (next.phase !== 'stopped') recording.preview = null
  recording.status = next
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
  recording.status = IDLE
  recording.preview = null
  recording.closedByHand = false
}
