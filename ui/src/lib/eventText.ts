// Plain words for the event log. The backend (router-core `event_text`) sends an i18n key and
// typed parameters for each event; this turns them into a sentence in the UI's language.
import type { ChangeGroup } from './bindings/ChangeGroup'
import type { ChangeOutcome } from './bindings/ChangeOutcome'
import type { EventParam } from './bindings/EventParam'
import type { Severity } from './bindings/Severity'
import { formatNumber, isolate, type DigitStyle } from './format'
import { t, type MessageKey } from './i18n'

/**
 * `t` for a key that came from the backend. A key this version has no text for (a newer backend,
 * or a value it didn't expect) shows as the key itself instead of breaking the page.
 */
export function translate(key: string, vars?: Record<string, string | number>): string {
  try {
    return t(key as MessageKey, vars)
  } catch {
    return key
  }
}

/** "B1 + B3", or "n41 + n78", with the user's digits, kept left to right inside Arabic text. */
export function bandList(bands: readonly number[], digits: DigitStyle, prefix = 'B'): string {
  if (!bands.length) return t('history.marker.otherBands')
  return isolate(bands.map((b) => `${prefix}${formatNumber(b, { digits })}`).join(' + '))
}

export function paramText(p: EventParam, digits: DigitStyle): string {
  switch (p.t) {
    case 'text':
      return isolate(p.v)
    case 'number':
      return formatNumber(p.v, { digits })
    case 'key':
      return translate(p.v)
    case 'bands':
      return bandList(p.v, digits)
    case 'nrBands':
      return bandList(p.v, digits, 'n')
  }
}

/** One event's sentence: its key through `t`, with each parameter in words. */
export function renderEventText(
  key: string,
  params: { [name: string]: EventParam | undefined },
  digits: DigitStyle,
): string {
  const vars: Record<string, string> = {}
  for (const [name, p] of Object.entries(params)) if (p) vars[name] = paramText(p, digits)
  return translate(key, vars)
}

export type OutcomeName = ChangeOutcome['kind']

/** The heading of a band change: what was tried and how it ended. */
export function changeTitle(g: ChangeGroup, digits: DigitStyle): string {
  return translate(`events.change.${g.outcome.kind}`, { from: bandList(g.from, digits), to: bandList(g.to, digits) })
}

/** How a change ended, as a severity: the CSV export uses the same mapping. */
export function changeSeverity(o: ChangeOutcome): Severity {
  switch (o.kind) {
    case 'open':
      return 'info'
    case 'kept':
      return 'good'
    case 'reverted':
    case 'rejected':
      return 'warning'
    case 'revertFailed':
      return 'problem'
  }
}

/**
 * The CSV export writes a whole band change in one cell from `events.change.<outcome>`, with
 * `{from}`, `{to}` and `{reason}` filled in by the backend. The card shows the reason on a line of
 * its own, so for the CSV the reverted text gets that line added.
 */
export function csvChangeTemplate(outcome: OutcomeName): string {
  const base = t(`events.change.${outcome}`)
  return outcome === 'reverted' ? `${base}. ${t('events.whyLine')}` : base
}
