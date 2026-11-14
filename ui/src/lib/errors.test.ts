import { describe, expect, it } from 'vitest'
import { errorText } from './errors'
import { t } from './i18n'

describe('errorText', () => {
  it('maps the backend change-in-progress refusal', () => {
    expect(errorText('Finish or revert the current band change before changing router or driver.')).toBe(
      t('settings.error.changeInProgress'),
    )
  })
  it('maps the other refusals of a router or driver change, and of an APN switch with data off', () => {
    expect(errorText('Wait for the IP refresh to finish before changing router or driver.')).toBe(
      t('settings.error.ipRefreshInProgress'),
    )
    expect(errorText('Finish or undo the APN change before changing router or driver.')).toBe(t('settings.error.apnInProgress'))
    expect(errorText({ kind: 'validation', message: 'mobile data is off on the router' })).toBe(t('network.apn.refuse.dataOff'))
  })
  it('maps RouterError validation objects by message', () => {
    expect(errorText({ kind: 'validation', message: 'the reboot confirmation is invalid or expired' })).toBe(
      t('reboot.expired'),
    )
  })
  it('maps the refusal to connect while a router recording is open or waiting', () => {
    // The backend's own words, from `recording::refuse_while_open`.
    const said = "A recording of the router's page is open. Stop or discard it first."
    expect(errorText(said)).toBe(t('errors.recordingOpen'))
    expect(t('errors.recordingOpen')).not.toContain(said)
  })
  it('maps the history refusals', () => {
    expect(errorText('wait for the auto-scan or speed test to finish')).toBe(t('errors.historyBusy'))
    expect(errorText('history is not available')).toBe(t('errors.historyUnavailable'))
    expect(errorText('history was made by a newer version of the app')).toBe(t('errors.historyNewer'))
  })
  it('names the router code on a lost session when there is one', () => {
    expect(errorText({ kind: 'sessionLost' })).toBe(t('errors.sessionLost'))
    const coded = errorText({ kind: 'sessionLost', code: 100003 })
    expect(coded).toBe(t('errors.sessionLostCode', { code: '100003' }))
    expect(coded).toContain('(code 100003)')
  })
  it('falls back to a generic message with the raw text', () => {
    expect(errorText('boom')).toContain('boom')
    expect(errorText(new Error('bang'))).toContain('bang')
  })
})

describe('APN refusals', () => {
  it('maps every APN refusal the backend can send', async () => {
    const { readFileSync } = await import('node:fs')
    const { resolve } = await import('node:path')
    const src = readFileSync(resolve(__dirname, '../../../crates/router-core/src/apn.rs'), 'utf8')
    const consts = [...src.matchAll(/pub const (APN_\w+): &str =\s*"([^"]*)";/g)]
      .filter(([, name]) => !['APN_JOURNAL_FILE', 'APN_ARCHIVE_DIR', 'APN_NOT_SUPPORTED'].includes(name))
      .map(([, name, text]) => [name, text])
    expect(consts.length).toBeGreaterThan(20)
    for (const [name, text] of consts) {
      expect(errorText(text), name).not.toBe(t('app.error', { message: text }))
    }
  })
})
