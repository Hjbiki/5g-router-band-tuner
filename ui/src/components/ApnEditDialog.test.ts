import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/svelte'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))

import { invoke } from '@tauri-apps/api/core'
import { app } from '../lib/store.svelte'
import { setLanguage, t } from '../lib/i18n'
import { settingsFixture, snap } from '../test-fixtures'
import { apnDraft, apnEditRefusal, apnFormProblem, profileEditRefusal, type ApnForm } from '../lib/apn'
import type { ApnProfile } from '../lib/bindings/ApnProfile'
import type { ApnProfiles as Profiles } from '../lib/bindings/ApnProfiles'
import type { AppSnapshot } from '../lib/bindings/AppSnapshot'
import ApnEditDialog from './ApnEditDialog.svelte'
import ApnProfiles from './ApnProfiles.svelte'

const SECRET = 'typed-secret-5678'

function profile(o: Partial<ApnProfile>): ApnProfile {
  return {
    index: 1,
    name: 'Operator internet',
    apn: 'internet.example',
    apnStatic: true,
    username: '',
    hasPassword: false,
    auth: 'auto',
    ipType: 'ipv4v6',
    readOnly: false,
    valid: true,
    fingerprint: '0123456789abcdef',
    ...o,
  }
}

const WORK = profile({ index: 2, name: 'Work', apn: 'work.example', username: 'demo', hasPassword: true, auth: 'chap', ipType: 'ipv4', fingerprint: 'aaaaaaaaaaaaaaaa' })

const LIST: Profiles = {
  current: 1,
  autoApn: false,
  profiles: [profile({ index: 1, readOnly: true }), WORK, profile({ index: 3, name: 'Old', valid: false })],
}

function editable(o: Partial<AppSnapshot> = {}): AppSnapshot {
  const s = snap({ apn: LIST, ...o })
  s.capabilities = { ...s.capabilities!, apn: { read: true, switch: true, edit: true, confirmed: false } }
  return s
}

function form(o: Partial<ApnForm> = {}): ApnForm {
  return { name: 'Trip', apn: 'trip.example', username: '', password: '', removePassword: false, auth: 'auto', ipType: 'ipv4v6', ...o }
}

describe('APN form rules', () => {
  it('match the backend', () => {
    expect(apnFormProblem(form())).toBeNull()
    expect(apnFormProblem(form({ name: '  ' }))?.field).toBe('name')
    expect(apnFormProblem(form({ name: 'x'.repeat(65) }))?.field).toBe('name')
    expect(apnFormProblem(form({ apn: 'has space' }))?.field).toBe('apn')
    expect(apnFormProblem(form({ apn: '' }))).toBeNull()
    expect(apnFormProblem(form({ username: 'u'.repeat(65) }))?.field).toBe('username')
    expect(apnFormProblem(form({ password: 'a\nb' }))?.field).toBe('password')
  })

  it('a blank password keeps the saved one on an edit, and means none otherwise', () => {
    expect(apnDraft(form(), WORK).password).toEqual({ kind: 'keep' })
    expect(apnDraft(form(), null).password).toEqual({ kind: 'clear' })
    expect(apnDraft(form(), profile({ hasPassword: false })).password).toEqual({ kind: 'clear' })
    expect(apnDraft(form({ removePassword: true, password: 'x' }), WORK).password).toEqual({ kind: 'clear' })
    expect(apnDraft(form({ password: SECRET }), WORK).password).toEqual({ kind: 'set', value: SECRET })
    expect(apnDraft(form({ name: ' Trip ' }), null).name).toBe('Trip')
  })

  it('edit and delete are only for profiles not in use, not the operator\'s and complete', () => {
    expect(profileEditRefusal(LIST.profiles[0], 1)).toBe('network.apn.edit.why.inUse')
    expect(profileEditRefusal(profile({ index: 4, readOnly: true }), 1)).toBe('network.apn.edit.why.readOnly')
    expect(profileEditRefusal(LIST.profiles[2], 1)).toBe('network.apn.edit.why.invalid')
    expect(profileEditRefusal(WORK, 1)).toBeNull()
    expect(apnEditRefusal(editable(), { scan: false, speedTest: false })).toBeNull()
    expect(apnEditRefusal(editable(), { scan: false, speedTest: true })).toBe('network.apn.edit.why.busy')
    expect(apnEditRefusal(snap({ apn: LIST }), { scan: false, speedTest: false })).toBe('network.apn.edit.refuse.notSupported')
  })
})

describe('ApnEditDialog', () => {
  afterEach(() => {
    cleanup()
    app.snapshot = null
    vi.mocked(invoke).mockReset()
  })

  it('masks the password, shows it on request, and never keeps it after closing', async () => {
    app.settings = settingsFixture()
    app.snapshot = editable()
    const onclose = vi.fn()
    render(ApnEditDialog, { mode: 'edit', profile: WORK, current: LIST.profiles[0], onclose })
    const pw = screen.getByTestId('apn-ed-pw') as HTMLInputElement
    expect(pw.type).toBe('password')
    expect(pw.placeholder).toBe(t('network.apn.edit.passwordKeep'))
    await fireEvent.input(pw, { target: { value: SECRET } })
    await fireEvent.click(screen.getByTestId('apn-ed-show'))
    expect(pw.type).toBe('text')
    await fireEvent.click(screen.getByTestId('apn-ed-cancel'))
    expect(onclose).toHaveBeenCalled()
    expect(pw.value).toBe('')
    expect(JSON.stringify(app.snapshot)).not.toContain(SECRET)
  })

  it('checks the fields before sending anything', async () => {
    app.settings = settingsFixture()
    app.snapshot = editable()
    render(ApnEditDialog, { mode: 'add', profile: null, current: LIST.profiles[0], onclose: vi.fn() })
    await fireEvent.input(screen.getByTestId('apn-ed-name'), { target: { value: 'Trip' } })
    await fireEvent.input(screen.getByTestId('apn-ed-apn'), { target: { value: 'not valid!' } })
    await fireEvent.click(screen.getByTestId('apn-ed-submit'))
    expect(screen.getByRole('alert').textContent).toBe(t('network.apn.edit.invalid.apn'))
    expect(invoke).not.toHaveBeenCalled()
  })

  it('adds a profile with the typed password and a toast without it', async () => {
    app.settings = settingsFixture()
    app.snapshot = editable()
    app.toasts = []
    vi.mocked(invoke).mockResolvedValue(4)
    const onclose = vi.fn()
    render(ApnEditDialog, { mode: 'add', profile: null, current: LIST.profiles[0], onclose })
    await fireEvent.input(screen.getByTestId('apn-ed-name'), { target: { value: 'Trip' } })
    await fireEvent.input(screen.getByTestId('apn-ed-apn'), { target: { value: 'trip.example' } })
    await fireEvent.input(screen.getByTestId('apn-ed-pw'), { target: { value: SECRET } })
    await fireEvent.click(screen.getByTestId('apn-ed-submit'))
    expect(invoke).toHaveBeenCalledWith('apn_add', {
      draft: { name: 'Trip', apn: 'trip.example', username: '', password: { kind: 'set', value: SECRET }, auth: 'auto', ipType: 'ipv4v6' },
    })
    await vi.waitFor(() => expect(onclose).toHaveBeenCalled())
    expect(JSON.stringify(app.toasts)).not.toContain(SECRET)
    expect(app.toasts.some((x) => x.text === t('network.apn.edit.added'))).toBe(true)
  })

  it('shows a refusal in words, keeps the dialog, and never the password', async () => {
    app.settings = settingsFixture()
    app.snapshot = editable()
    vi.mocked(invoke).mockRejectedValue({
      kind: 'validation',
      message: "the router hides this profile's saved password, so it can't be kept",
    })
    const onclose = vi.fn()
    render(ApnEditDialog, { mode: 'edit', profile: WORK, current: LIST.profiles[0], onclose })
    await fireEvent.click(screen.getByTestId('apn-ed-submit'))
    expect(invoke).toHaveBeenCalledWith('apn_edit', {
      index: 2,
      fingerprint: 'aaaaaaaaaaaaaaaa',
      draft: { name: 'Work', apn: 'work.example', username: 'demo', password: { kind: 'keep' }, auth: 'chap', ipType: 'ipv4' },
    })
    await vi.waitFor(() =>
      expect(screen.getByTestId('apn-ed-error').textContent).toBe(t('network.apn.edit.refuse.passwordHidden')),
    )
    expect(onclose).not.toHaveBeenCalled()
  })

  it('"Remove password" sends a clear and empties the field', async () => {
    app.settings = settingsFixture()
    app.snapshot = editable()
    vi.mocked(invoke).mockResolvedValue(undefined)
    render(ApnEditDialog, { mode: 'edit', profile: WORK, current: LIST.profiles[0], onclose: vi.fn() })
    const pw = screen.getByTestId('apn-ed-pw') as HTMLInputElement
    await fireEvent.input(pw, { target: { value: SECRET } })
    await fireEvent.click(screen.getByTestId('apn-ed-remove'))
    expect(pw.value).toBe('')
    expect(pw.disabled).toBe(true)
    await fireEvent.click(screen.getByTestId('apn-ed-submit'))
    expect(vi.mocked(invoke).mock.calls[0][1]).toMatchObject({ draft: { password: { kind: 'clear' } } })
  })

  it('deletes after one confirmation that names the profile', async () => {
    app.settings = settingsFixture()
    app.snapshot = editable()
    vi.mocked(invoke).mockResolvedValue(undefined)
    render(ApnEditDialog, { mode: 'delete', profile: WORK, current: LIST.profiles[0], onclose: vi.fn() })
    expect(screen.getByTestId('apn-del-body').textContent).toContain('Work')
    expect(screen.queryByTestId('apn-ed-pw')).toBeNull()
    expect(invoke).not.toHaveBeenCalled()
    await fireEvent.click(screen.getByTestId('apn-ed-submit'))
    expect(invoke).toHaveBeenCalledWith('apn_delete', { index: 2, fingerprint: 'aaaaaaaaaaaaaaaa' })
  })

  it('has text in Arabic too', () => {
    setLanguage('ar')
    try {
      for (const k of ['add', 'addTitle', 'editTitle', 'deleteTitle', 'deleteBody', 'passwordKeep', 'removePassword', 'save', 'note'] as const) {
        expect(t(`network.apn.edit.${k}`)).not.toBe(`network.apn.edit.${k}`)
      }
    } finally {
      setLanguage('en')
    }
  })
})

describe('ApnProfiles edit buttons', () => {
  afterEach(() => {
    cleanup()
    app.snapshot = null
    vi.mocked(invoke).mockReset()
  })

  it('shows Edit and Delete only where they can be used, and says why elsewhere', async () => {
    app.settings = settingsFixture()
    app.snapshot = editable()
    render(ApnProfiles)
    const rows = screen.getAllByTestId('apn-row')
    expect(rows[0].querySelector('[data-testid="apn-edit"]')).toBeNull()
    expect(rows[0].querySelector('[data-testid="apn-edit-why"]')?.getAttribute('title')).toBe(t('network.apn.edit.why.inUse'))
    expect(rows[2].querySelector('[data-testid="apn-edit-why"]')?.getAttribute('title')).toBe(t('network.apn.edit.why.invalid'))
    expect(rows[1].querySelector('[data-testid="apn-edit"]')).not.toBeNull()
    await fireEvent.click(rows[1].querySelector('[data-testid="apn-delete"]') as HTMLButtonElement)
    expect(screen.getByTestId('apn-edit-dialog').textContent).toContain('Work')
    await fireEvent.click(screen.getByTestId('apn-ed-cancel'))
    await fireEvent.click(screen.getByTestId('apn-add'))
    expect(screen.getByTestId('apn-ed-name')).toBeTruthy()
  })

  it('never edits a profile with a setting the app does not know, but may delete it', async () => {
    app.settings = settingsFixture()
    const odd = profile({ index: 4, name: 'Odd', apn: 'odd.example', auth: 'other', fingerprint: 'bbbbbbbbbbbbbbbb' })
    app.snapshot = editable({ apn: { ...LIST, profiles: [...LIST.profiles, odd] } })
    render(ApnProfiles)
    const row = screen.getAllByTestId('apn-row')[3]
    expect(row.querySelector('[data-testid="apn-edit"]')).toBeNull()
    expect(row.querySelector('[data-testid="apn-edit-why"]')?.getAttribute('title')).toBe(t('network.apn.edit.why.unknownSetting'))
    expect(row.querySelector('[data-testid="apn-delete"]')).not.toBeNull()
    cleanup()
    // The dialog itself refuses too, so a rename can't rewrite the sign-in method or IP type.
    render(ApnEditDialog, { mode: 'edit', profile: { ...odd, auth: 'auto', ipType: 'other' }, current: LIST.profiles[0], onclose: () => {} })
    expect(screen.getByTestId('apn-ed-refusal').textContent).toBe(t('network.apn.edit.why.unknownSetting'))
    expect((screen.getByTestId('apn-ed-submit') as HTMLButtonElement).disabled).toBe(true)
    expect(invoke).not.toHaveBeenCalled()
  })

  it('has no edit controls on a router that can\'t edit', () => {
    app.settings = settingsFixture()
    app.snapshot = snap({ apn: LIST })
    render(ApnProfiles)
    expect(screen.queryByTestId('apn-add')).toBeNull()
    expect(screen.queryByTestId('apn-edit')).toBeNull()
    expect(screen.queryByTestId('apn-edit-why')).toBeNull()
  })
})
