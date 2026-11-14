// APN switch helpers: whether one is open, and why "Use this APN" can't start right now.
import type { AppSnapshot } from './bindings/AppSnapshot'
import type { ApnProfile } from './bindings/ApnProfile'
import type { ApnDraft } from './bindings/ApnDraft'
import type { MessageKey } from './i18n'
import { changeClosed, changeOpen } from './bands'

/** An APN switch is being applied or watched (the backend refuses everything else meanwhile). */
export function apnChangeOpen(s: AppSnapshot | null | undefined): boolean {
  const v = s?.apnApply
  return v != null && !changeClosed(v.phase)
}

/** Why no profile can be switched to now, for the inline reason. `busy` names the UI's own scan or speed test. */
export function apnSwitchRefusal(
  s: AppSnapshot | null | undefined,
  busy: { scan: boolean; speedTest: boolean },
): MessageKey | null {
  if (!s || s.session.kind !== 'loggedIn') return 'network.apn.why.notConnected'
  if (!s.capabilities?.apn.switch) return 'network.apn.refuse.notSupported'
  if (!s.apn) return 'network.apn.refuse.unknown'
  if (s.apn.autoApn === true) return 'network.apn.refuse.autoOn'
  if (apnChangeOpen(s)) return 'network.apn.refuse.running'
  if (changeOpen(s.safeApply)) return 'network.apn.why.bandChange'
  if (s.ipRefresh?.active) return 'network.apn.why.ipRefresh'
  if (busy.scan) return 'network.apn.why.scan'
  if (busy.speedTest) return 'network.apn.why.speedTest'
  return null
}

/** Why this one profile can't be chosen, or null when it can. */
export function profileRefusal(p: ApnProfile, current: number | null | undefined): MessageKey | null {
  if (p.index === current) return 'network.apn.why.inUse'
  if (!p.valid) return 'network.apn.refuse.invalid'
  return null
}

/** Why no profile can be added, edited or deleted now, or null when they can. */
export function apnEditRefusal(
  s: AppSnapshot | null | undefined,
  busy: { scan: boolean; speedTest: boolean },
): MessageKey | null {
  if (!s || s.session.kind !== 'loggedIn') return 'network.apn.edit.why.notConnected'
  if (!s.capabilities?.apn.edit) return 'network.apn.edit.refuse.notSupported'
  if (!s.apn || s.apn.current == null) return 'network.apn.refuse.unknown'
  if (apnChangeOpen(s)) return 'network.apn.refuse.running'
  if (changeOpen(s.safeApply) || s.ipRefresh?.active || busy.scan || busy.speedTest) return 'network.apn.edit.why.busy'
  return null
}

/** Why this profile can't be edited or deleted (the buttons are hidden), or null when it can. */
export function profileEditRefusal(p: ApnProfile, current: number | null | undefined): MessageKey | null {
  if (p.index === current) return 'network.apn.edit.why.inUse'
  if (p.readOnly) return 'network.apn.edit.why.readOnly'
  if (!p.valid) return 'network.apn.edit.why.invalid'
  return null
}

/** Why a profile that may be deleted still can't be edited: it uses a sign-in method or IP type
 * the app doesn't know, which the form can't show, so saving it would silently change it. */
export function profileOnlyEditRefusal(p: ApnProfile): MessageKey | null {
  return p.auth === 'other' || p.ipType === 'other' ? 'network.apn.edit.why.unknownSetting' : null
}

/** The dialog's fields. The password stays in the dialog and is cleared when it closes. */
export interface ApnForm {
  name: string
  apn: string
  username: string
  password: string
  removePassword: boolean
  auth: ApnDraft['auth']
  ipType: ApnDraft['ipType']
}

export type ApnFormField = 'name' | 'apn' | 'username' | 'password'

const CONTROL = /[\u0000-\u001f\u007f-\u009f]/
const APN_TEXT = /^[A-Za-z0-9.-]{0,100}$/

/** The same rules as the backend; the first field that breaks one, with its message. */
export function apnFormProblem(f: ApnForm): { field: ApnFormField; key: MessageKey } | null {
  const name = f.name.trim()
  const len = (s: string) => [...s].length
  if (len(name) === 0 || len(name) > 64 || CONTROL.test(name)) return { field: 'name', key: 'network.apn.edit.invalid.name' }
  if (!APN_TEXT.test(f.apn.trim())) return { field: 'apn', key: 'network.apn.edit.invalid.apn' }
  const user = f.username.trim()
  if (len(user) > 64 || CONTROL.test(user)) return { field: 'username', key: 'network.apn.edit.invalid.username' }
  if (len(f.password) > 64 || CONTROL.test(f.password)) return { field: 'password', key: 'network.apn.edit.invalid.password' }
  return null
}

/**
 * The draft to send. A blank password keeps the saved one on an edit (when there is one) and
 * means none otherwise; "Remove" clears it.
 */
export function apnDraft(f: ApnForm, editing: ApnProfile | null): ApnDraft {
  let password: ApnDraft['password']
  if (f.removePassword) password = { kind: 'clear' }
  else if (f.password !== '') password = { kind: 'set', value: f.password }
  else if (editing?.hasPassword) password = { kind: 'keep' }
  else password = { kind: 'clear' }
  return {
    name: f.name.trim(),
    apn: f.apn.trim(),
    username: f.username.trim(),
    password,
    auth: f.auth,
    ipType: f.ipType,
  }
}
