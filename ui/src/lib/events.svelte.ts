// The History page's event log: its filters, the entries loaded so far and "Show older".
import type { EventCategory } from './bindings/EventCategory'
import type { EventEntry } from './bindings/EventEntry'
import type { EventQuery } from './bindings/EventQuery'
import type { HistoryRange } from './bindings/HistoryRange'
import { api } from './backend'
import { errorText } from './errors'

export type EventFilter = 'all' | 'bandChange' | 'connection' | 'speedTest' | 'other'
export const EVENT_FILTERS: readonly EventFilter[] = ['all', 'bandChange', 'connection', 'speedTest', 'other']

/** The categories each filter asks for; none means every category. */
export const FILTER_CATEGORIES: Record<EventFilter, EventCategory[]> = {
  all: [],
  bandChange: ['bandChange'],
  connection: ['connection'],
  speedTest: ['speedTest'],
  other: ['device', 'app'],
}

/** Entries per page. */
export const EVENTS_PAGE = 50

export const eventLog: {
  filter: EventFilter
  problemsOnly: boolean
  technical: boolean
  entries: EventEntry[]
  /** Pass for the next page; null when there is nothing older. */
  nextBeforeMs: number | null
  loading: boolean
  error: string | null
  /** `eventsKey` of what the entries show; null before the first load. */
  loadedKey: string | null
} = $state({
  filter: 'all',
  problemsOnly: false,
  technical: false,
  entries: [],
  nextBeforeMs: null,
  loading: false,
  error: null,
  loadedKey: null,
})

export function eventQuery(range: HistoryRange, beforeMs: number | null = null): EventQuery {
  return {
    range,
    beforeMs,
    limit: EVENTS_PAGE,
    categories: FILTER_CATEGORIES[eventLog.filter],
    problemsOnly: eventLog.problemsOnly,
    technical: eventLog.technical,
  }
}

let request = 0
let inflight: { key: string; promise: Promise<boolean> } | null = null

/** What the first page for `range` with the current filters is asked with, as a string. */
export function eventsKey(range: HistoryRange): string {
  return JSON.stringify(eventQuery(range, null))
}

/**
 * Loads the first page for `range` with the current filters, or with `older` the page after the
 * entries already shown. The same request already on its way is shared, and an answer for an
 * older request is dropped. Resolves true when this answer is what the log now shows.
 */
export function loadEvents(range: HistoryRange, older = false): Promise<boolean> {
  const before = older ? eventLog.nextBeforeMs : null
  if (older && before === null) return Promise.resolve(false)
  const q = eventQuery(range, before)
  const key = JSON.stringify(q)
  if (inflight?.key === key) return inflight.promise
  const id = ++request
  const first = eventsKey(range)
  eventLog.loading = true
  let settled = false
  const promise = (async () => {
    try {
      const page = await api.historyEvents(q)
      if (id !== request) return false
      eventLog.entries = older ? [...eventLog.entries, ...page.entries] : page.entries
      eventLog.nextBeforeMs = page.nextBeforeMs
      eventLog.loadedKey = first
      eventLog.error = null
      return true
    } catch (e) {
      if (id !== request) return false
      eventLog.error = errorText(e)
      return false
    } finally {
      settled = true
      if (id === request) eventLog.loading = false
      if (inflight?.key === key) inflight = null
    }
  })()
  if (!settled) inflight = { key, promise }
  return promise
}

/** Test hook: back to the defaults, dropping any answer still on its way. */
export function resetEvents(): void {
  request++
  inflight = null
  Object.assign(eventLog, {
    filter: 'all',
    problemsOnly: false,
    technical: false,
    entries: [],
    nextBeforeMs: null,
    loading: false,
    error: null,
    loadedKey: null,
  })
}
