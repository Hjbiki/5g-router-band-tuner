// The words the CSV export needs, in the UI's language (or English when asked): the backend
// writes the file but has no translations of its own.
import type { CsvLabels } from './bindings/CsvLabels'
import { currentLanguage, messagesUnder } from './i18n'

/** Event-log texts the backend may name by key: every key under these prefixes is sent. */
export const CSV_TEXT_PREFIXES = [
  'events.kind',
  'events.change',
  'events.why',
  'events.value',
  'safeApply.reason',
  'speedtest.fail',
  'history.export.dataset',
] as const

/**
 * Headers from `history.csv.col.*`, values from `history.csv.value.*`, and the event texts. A
 * reverted change is written in one cell, so its text gets the reason line added.
 */
export function csvLabels(english: boolean): CsvLabels {
  const lang = english ? 'en' : currentLanguage()
  const texts: Record<string, string> = {}
  for (const prefix of CSV_TEXT_PREFIXES) {
    for (const [k, v] of Object.entries(messagesUnder(prefix, lang))) texts[`${prefix}.${k}`] = v
  }
  const whyLine = messagesUnder('events', lang).whyLine
  if (texts['events.change.reverted'] && whyLine) {
    texts['events.change.reverted'] = `${texts['events.change.reverted']}. ${whyLine}`
  }
  return {
    headers: messagesUnder('history.csv.col', lang),
    values: messagesUnder('history.csv.value', lang),
    texts,
  }
}
