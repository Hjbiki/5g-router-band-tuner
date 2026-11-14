<script lang="ts">
  // Add or edit an APN profile that isn't in use, or delete one after a confirmation naming it.
  // The password lives in this component only: it is masked by default, never put in the store,
  // a toast or an error, and cleared whenever the dialog closes.
  import Eye from '@lucide/svelte/icons/eye'
  import EyeOff from '@lucide/svelte/icons/eye-off'
  import Trash2 from '@lucide/svelte/icons/trash-2'
  import Waypoints from '@lucide/svelte/icons/waypoints'
  import { onDestroy } from 'svelte'
  import { api } from '../lib/backend'
  import { app, pushToast } from '../lib/store.svelte'
  import { t } from '../lib/i18n'
  import { errorText } from '../lib/errors'
  import { isolateText as fsi } from '../lib/format'
  import { apnDraft, apnEditRefusal, apnFormProblem, profileOnlyEditRefusal, type ApnForm, type ApnFormField } from '../lib/apn'
  import { scanActive } from '../lib/scan.svelte'
  import { speed } from '../lib/speedtest.svelte'
  import type { ApnProfile } from '../lib/bindings/ApnProfile'

  let {
    mode,
    profile = null,
    current = null,
    onclose,
  }: {
    mode: 'add' | 'edit' | 'delete'
    profile?: ApnProfile | null
    current?: ApnProfile | null
    onclose: () => void
  } = $props()

  const editing = $derived(mode === 'edit' ? profile : null)
  /** The fields as the dialog opens; later prop changes don't reset what the user typed. */
  function initialForm(): ApnForm {
    const p = mode === 'add' ? null : profile
    return {
      name: p?.name ?? '',
      apn: p?.apn ?? '',
      username: p?.username ?? '',
      password: '',
      removePassword: false,
      // An edit of a profile with a setting the app doesn't know is refused below.
      auth: p && p.auth !== 'other' ? p.auth : 'auto',
      ipType: p && p.ipType !== 'other' ? p.ipType : 'ipv4v6',
    }
  }
  let form: ApnForm = $state(initialForm())
  let show = $state(false)
  let busy = $state(false)
  let error: string | null = $state(null)
  let touched = $state(false)
  let first: HTMLInputElement | undefined = $state()
  let cancelBtn: HTMLButtonElement | undefined = $state()

  const refusal = $derived(
    (editing ? profileOnlyEditRefusal(editing) : null) ??
      apnEditRefusal(app.snapshot, { scan: scanActive(), speedTest: speed.running }),
  )
  const problem = $derived(mode === 'delete' ? null : apnFormProblem(form))
  const name = $derived(profile ? profile.name || t('network.apn.unnamed') : '')
  const currentName = $derived(current ? current.name || t('network.apn.unnamed') : t('network.apn.current'))
  const keepHint = $derived(editing?.hasPassword === true && !form.removePassword)
  const title = $derived(
    mode === 'add' ? t('network.apn.edit.addTitle') : mode === 'edit' ? t('network.apn.edit.editTitle') : t('network.apn.edit.deleteTitle'),
  )

  function invalid(f: ApnFormField): boolean {
    return touched && problem?.field === f
  }

  /** Forgets the typed password, then closes. */
  function close() {
    form.password = ''
    show = false
    onclose()
  }

  onDestroy(() => {
    form.password = ''
  })

  $effect(() => {
    queueMicrotask(() => (first ?? cancelBtn)?.focus())
  })

  async function submit() {
    if (busy || refusal) return
    touched = true
    if (problem) return
    busy = true
    error = null
    try {
      if (mode === 'delete' && profile) {
        await api.apnDelete(profile.index, profile.fingerprint)
        pushToast(t('network.apn.edit.deleted'))
      } else if (mode === 'edit' && profile) {
        await api.apnEdit(profile.index, profile.fingerprint, apnDraft(form, profile))
        pushToast(t('network.apn.edit.saved'))
      } else {
        await api.apnAdd(apnDraft(form, null))
        pushToast(t('network.apn.edit.added'))
      }
      close()
    } catch (e) {
      // The backend's refusals never hold the password; only their mapped text is shown.
      error = errorText(e)
    } finally {
      busy = false
    }
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div class="backdrop" role="presentation" onkeydown={(e) => e.key === 'Escape' && close()}>
  <div class="modal" role="dialog" aria-modal="true" aria-labelledby="apn-ed-title" data-testid="apn-edit-dialog">
    <h2 id="apn-ed-title">
      {#if mode === 'delete'}<Trash2 size={18} aria-hidden="true" />{:else}<Waypoints size={18} aria-hidden="true" />{/if}{title}
    </h2>
    {#if mode === 'delete'}
      <p data-testid="apn-del-body">{t('network.apn.edit.deleteBody', { name: fsi(name) })}</p>
    {:else}
      <form
        class="fields"
        novalidate
        onsubmit={(e) => {
          e.preventDefault()
          void submit()
        }}
      >
        <label class="f">
          <span>{t('network.apn.edit.name')}</span>
          <input
            bind:this={first}
            bind:value={form.name}
            maxlength="64"
            autocomplete="off"
            spellcheck="false"
            aria-invalid={invalid('name')}
            data-testid="apn-ed-name"
          />
          {#if invalid('name') && problem}<small class="err" role="alert">{t(problem.key)}</small>{/if}
        </label>
        <label class="f">
          <span>{t('network.apn.edit.apn')}</span>
          <input
            bind:value={form.apn}
            dir="ltr"
            maxlength="100"
            autocomplete="off"
            spellcheck="false"
            aria-invalid={invalid('apn')}
            aria-describedby="apn-ed-apn-hint"
            data-testid="apn-ed-apn"
          />
          <small id="apn-ed-apn-hint" class="hint">{t('network.apn.edit.apnHint')}</small>
          {#if invalid('apn') && problem}<small class="err" role="alert">{t(problem.key)}</small>{/if}
        </label>
        <label class="f">
          <span>{t('network.apn.edit.username')}</span>
          <input
            bind:value={form.username}
            dir="ltr"
            maxlength="64"
            autocomplete="off"
            spellcheck="false"
            aria-invalid={invalid('username')}
            data-testid="apn-ed-user"
          />
          {#if invalid('username') && problem}<small class="err" role="alert">{t(problem.key)}</small>{/if}
        </label>
        <div class="f">
          <label for="apn-ed-pw">{t('network.apn.edit.password')}</label>
          <div class="pw">
            <input
              id="apn-ed-pw"
              type={show ? 'text' : 'password'}
              dir="ltr"
              bind:value={form.password}
              maxlength="64"
              autocomplete="new-password"
              spellcheck="false"
              placeholder={keepHint ? t('network.apn.edit.passwordKeep') : undefined}
              disabled={form.removePassword}
              aria-invalid={invalid('password')}
              data-testid="apn-ed-pw"
            />
            <button
              type="button"
              class="eye"
              aria-pressed={show}
              aria-label={t('network.apn.edit.showPassword')}
              aria-controls="apn-ed-pw"
              onclick={() => (show = !show)}
              data-testid="apn-ed-show"
              >{#if show}<EyeOff size={16} aria-hidden="true" />{:else}<Eye size={16} aria-hidden="true" />{/if}</button
            >
          </div>
          {#if keepHint}<small class="hint">{t('network.apn.edit.passwordKeep')}</small>{/if}
          {#if invalid('password') && problem}<small class="err" role="alert">{t(problem.key)}</small>{/if}
          {#if editing?.hasPassword}
            <label class="check">
              <input
                type="checkbox"
                bind:checked={form.removePassword}
                onchange={() => (form.password = '')}
                data-testid="apn-ed-remove"
              />{t('network.apn.edit.removePassword')}
            </label>
          {/if}
        </div>
        <div class="row">
          <label class="f">
            <span>{t('network.apn.edit.auth')}</span>
            <select bind:value={form.auth} data-testid="apn-ed-auth">
              <option value="auto">{t('network.apn.edit.authAuto')}</option>
              <option value="pap">{t('network.apn.edit.authPap')}</option>
              <option value="chap">{t('network.apn.edit.authChap')}</option>
            </select>
          </label>
          <label class="f">
            <span>{t('network.apn.edit.ipType')}</span>
            <select bind:value={form.ipType} data-testid="apn-ed-ip">
              <option value="ipv4">{t('network.apn.ipType.ipv4')}</option>
              <option value="ipv6">{t('network.apn.ipType.ipv6')}</option>
              <option value="ipv4v6">{t('network.apn.ipType.ipv4v6')}</option>
            </select>
          </label>
        </div>
        <p class="note">{t('network.apn.edit.note', { current: fsi(currentName) })}</p>
        <button type="submit" hidden aria-hidden="true" tabindex="-1"></button>
      </form>
    {/if}
    {#if refusal}
      <p class="why" role="status" data-testid="apn-ed-refusal">{t(refusal)}</p>
    {/if}
    {#if error}
      <p class="why" role="alert" data-testid="apn-ed-error">{error}</p>
    {/if}
    <div class="actions">
      <button type="button" bind:this={cancelBtn} onclick={close} data-testid="apn-ed-cancel">{t('app.cancel')}</button>
      <button
        type="button"
        class={mode === 'delete' ? 'danger' : 'primary'}
        disabled={busy || refusal !== null}
        onclick={() => void submit()}
        data-testid="apn-ed-submit"
        >{#if mode === 'delete'}{busy ? t('network.apn.edit.deleting') : t('network.apn.edit.delete')}{:else}{busy
            ? t('network.apn.edit.saving')
            : t('network.apn.edit.save')}{/if}</button
      >
    </div>
  </div>
</div>

<style>
  /* Below the title bar, so the window can still be moved and closed. */
  .backdrop {
    position: fixed;
    inset: 0;
    inset-block-start: var(--titlebar-height);
    display: grid;
    place-items: center;
    padding: 16px;
    background: var(--scrim);
    z-index: 40;
  }
  .modal {
    background: var(--surface);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: var(--radius-card);
    box-shadow: var(--shadow-pop);
    padding-block: 20px;
    padding-inline: 24px;
    inline-size: min(480px, 100%);
    max-block-size: 100%;
    overflow-y: auto;
    animation: pop-in var(--t-enter) var(--ease);
  }
  h2 {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: var(--fs-section);
    margin-block: 0 8px;
  }
  p {
    margin-block: 0 8px;
  }
  .fields {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .f {
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-inline-size: 0;
    flex: 1 1 10em;
  }
  .f > span,
  .f > label {
    font-weight: 500;
  }
  .row {
    display: flex;
    flex-wrap: wrap;
    gap: 10px;
  }
  .pw {
    display: flex;
    position: relative;
    min-inline-size: 0;
  }
  .pw input {
    flex: 1;
    min-inline-size: 0;
    padding-inline-end: 38px;
  }
  .eye {
    position: absolute;
    inset-block: 0;
    inset-inline-end: 0;
    display: inline-grid;
    place-items: center;
    inline-size: 36px;
    padding: 0;
    border: 0;
    background: transparent;
    color: var(--muted);
  }
  .check {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: var(--fs-small);
  }
  .hint,
  .note {
    color: var(--muted);
    font-size: var(--fs-small);
  }
  .err {
    color: var(--pill-negative-ink);
    font-size: var(--fs-small);
  }
  .why {
    color: var(--pill-negative-ink);
    background: var(--pill-negative-bg);
    border-radius: var(--radius-control);
    padding-block: 6px;
    padding-inline: 10px;
    margin-block-start: 8px;
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: 8px;
    margin-block-start: var(--gap);
  }
</style>
