import { describe, expect, it } from 'vitest'
import type { CredentialStatus } from './bindings/CredentialStatus'
import type { SessionView } from './bindings/SessionView'
import { loginShort, loginState, loginTone, needsAttention, type LoginState } from './login'
import { t } from './i18n'

const CREDS: [string, CredentialStatus | null][] = [
  ['unknown', null],
  ['none saved', { saved: false, rejected: false }],
  ['saved', { saved: true, rejected: false }],
  ['saved, rejected', { saved: true, rejected: true }],
]

// Every session kind except notConfigured maps the same whatever is saved: the session is newer.
const FROM_SESSION: [SessionView, LoginState][] = [
  [{ kind: 'loggedIn' }, { kind: 'connected' }],
  [{ kind: 'connecting' }, { kind: 'working', why: 'connecting' }],
  [{ kind: 'reconnecting', reason: 'timeout', retryInS: 12 }, { kind: 'working', why: 'retrying', retryInS: 12 }],
  [{ kind: 'takenOver' }, { kind: 'working', why: 'takenOver' }],
  [{ kind: 'badCredentials' }, { kind: 'wrongPassword' }],
  [{ kind: 'lockedOut', waitS: 90 }, { kind: 'locked', waitS: 90 }],
  [{ kind: 'algorithmMismatch' }, { kind: 'unsupported' }],
  [{ kind: 'loginRejected', code: 125001 }, { kind: 'ready', why: 'token', code: 125001 }],
  [{ kind: 'loginRejected', code: 125002 }, { kind: 'ready', why: 'token', code: 125002 }],
  [{ kind: 'loginRejected', code: 125003 }, { kind: 'ready', why: 'token', code: 125003 }],
  [{ kind: 'loginRejected', code: 125004 }, { kind: 'wrongPassword', code: 125004 }],
  [{ kind: 'loginRejected', code: 108009 }, { kind: 'wrongPassword', code: 108009 }],
  [{ kind: 'confirmRouter', firstTime: true }, { kind: 'confirmRouter', firstTime: true }],
  [{ kind: 'confirmRouter', firstTime: false }, { kind: 'confirmRouter', firstTime: false }],
  // Logged out on purpose: not "not set up", whatever is saved.
  [{ kind: 'loggedOut' }, { kind: 'loggedOut' }],
]

const NOT_CONFIGURED: Record<string, LoginState> = {
  unknown: { kind: 'loading' },
  'none saved': { kind: 'needPassword' },
  saved: { kind: 'ready', why: 'saved' },
  // Rejected in an earlier run: not "not set up", and nothing to retry without typing.
  'saved, rejected': { kind: 'wrongPassword' },
}

describe('loginState', () => {
  it('is loading until the first snapshot', () => {
    for (const [, cred] of CREDS) expect(loginState(null, cred)).toEqual({ kind: 'loading' })
  })

  for (const [name, cred] of CREDS) {
    it(`maps every session kind with credentials ${name}`, () => {
      for (const [view, want] of FROM_SESSION) expect(loginState(view, cred), view.kind).toEqual(want)
      expect(loginState({ kind: 'notConfigured' }, cred)).toEqual(NOT_CONFIGURED[name])
    })
  }
})

describe('loginShort, loginTone and needsAttention', () => {
  const all: LoginState[] = [
    { kind: 'loading' },
    { kind: 'connected' },
    { kind: 'working', why: 'connecting' },
    { kind: 'needPassword' },
    { kind: 'wrongPassword' },
    { kind: 'locked', waitS: 5 },
    { kind: 'confirmRouter', firstTime: true },
    { kind: 'ready', why: 'saved' },
    { kind: 'unsupported' },
    { kind: 'loggedOut' },
  ]

  it('names every state in a few words', () => {
    for (const s of all) expect(t(loginShort(s)).length, s.kind).toBeGreaterThan(0)
    expect(loginShort({ kind: 'needPassword' })).toBe('login.short.needPassword')
    expect(loginShort({ kind: 'wrongPassword' })).toBe('login.short.wrong')
    expect(loginShort({ kind: 'loggedOut' })).toBe('login.short.loggedOut')
    expect(loginTone({ kind: 'loggedOut' })).toBe('warn')
  })

  it('only the states that wait for the user need attention', () => {
    // Logged out shows the app-wide banner, so Connect is one click from any page.
    const want = ['needPassword', 'wrongPassword', 'locked', 'confirmRouter', 'ready', 'unsupported', 'loggedOut']
    expect(all.filter(needsAttention).map((s) => s.kind)).toEqual(want)
  })

  it('reads fine only when connected, and stopped when the password or model is the problem', () => {
    expect(all.filter((s) => loginTone(s) === 'ok').map((s) => s.kind)).toEqual(['connected'])
    expect(all.filter((s) => loginTone(s) === 'bad').map((s) => s.kind)).toEqual(['wrongPassword', 'locked', 'unsupported'])
  })
})
