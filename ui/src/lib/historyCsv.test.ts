import { afterEach, describe, expect, it } from 'vitest'
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { setLanguage, t } from './i18n'
import { csvLabels } from './historyCsv'

// Written by the Rust test `the_column_ids_are_written_for_the_ui` (src-tauri/src/history/csv.rs).
const columns = JSON.parse(readFileSync(resolve(__dirname, 'bindings/csvColumns.json'), 'utf8')) as {
  headers: string[]
  values: string[]
}

describe('csvLabels', () => {
  afterEach(() => setLanguage('en'))

  it('has a header for every column and a text for every value the backend writes, in both languages', () => {
    for (const lang of ['en', 'ar']) {
      setLanguage(lang)
      const labels = csvLabels(false)
      for (const id of columns.headers) expect(labels.headers[id], `${lang} header ${id}`).toBeTruthy()
      for (const id of columns.values) expect(labels.values[id], `${lang} value ${id}`).toBeTruthy()
    }
  })

  it('keeps the {offset} placeholder in the local time header', () => {
    setLanguage('ar')
    expect(csvLabels(false).headers.localTime).toContain('{offset}')
  })

  it('sends the event texts, with the reason line on a reverted change', () => {
    const { texts } = csvLabels(false)
    expect(texts['events.kind.appStarted']).toBe(t('events.kind.appStarted'))
    expect(texts['safeApply.reason.user']).toBe(t('safeApply.reason.user'))
    expect(texts['speedtest.fail.timeout']).toBe(t('speedtest.fail.timeout'))
    expect(texts['history.export.dataset.hourly']).toBe(t('history.export.dataset.hourly'))
    expect(texts['events.change.reverted']).toBe('Tried {to}, then went back to {from}. Why: {reason}')
  })

  it('gives English when asked, whatever the app language', () => {
    setLanguage('ar')
    const ar = csvLabels(false)
    const en = csvLabels(true)
    expect(en.headers.utcTime).toBe('UTC time')
    expect(ar.headers.utcTime).not.toBe('UTC time')
    expect(en.values['severity.problem']).toBe('Problem')
    expect(en.texts['events.change.kept']).toBe('Changed the bands to {to} and kept them (was {from})')
  })
})
