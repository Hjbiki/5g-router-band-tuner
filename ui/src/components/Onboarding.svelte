<script lang="ts">
  import { onDestroy } from 'svelte'
  import { app, updateSettings } from '../lib/store.svelte'
  import { LANGUAGES, resolveLanguage, setLanguage, t } from '../lib/i18n'
  import { errorText } from '../lib/errors'
  import { formatNumber } from '../lib/format'
  import { routerProbe } from '../lib/routerProbe.svelte'
  import type { Settings } from '../lib/bindings/Settings'
  import LoginPanel from './LoginPanel.svelte'
  import { refreshCred } from '../lib/credentials.svelte'

  const TOTAL = 3
  let step = $state(1)
  let busy = $state(false)
  let error: string | null = $state(null)
  let username = $state(app.settings?.username ?? 'admin')
  let editUser = $state(false)
  // Write-only: the panel sends it once and clears it straight away.
  let password = $state('')
  let checking = $state(false)
  let connected = $state(false)
  // Still no answer after this long: the guide can go on while the app keeps trying.
  const SLOW_MS = 25_000
  const OK_PAUSE_MS = 800
  let slow = $state(false)
  let panel: LoginPanel | undefined = $state()
  let timers: ReturnType<typeof setTimeout>[] = []
  onDestroy(() => timers.forEach(clearTimeout))
  $effect(() => {
    if (!checking) return
    slow = false
    const id = setTimeout(() => (slow = true), SLOW_MS)
    return () => clearTimeout(id)
  })
  let heading: HTMLHeadingElement | undefined = $state()
  // Off unless ticked: the update check only runs with the user's say-so.
  let allowUpdates = $state(false)

  const digits = $derived(app.settings?.digits ?? 'western')
  const lang = $derived(resolveLanguage(app.settings?.language))

  $effect(() => {
    void step
    heading?.focus()
  })

  /** Saves the current settings plus `patch`; the stored copy only changes on success. */
  async function persist(patch: Partial<Settings>) {
    await updateSettings((s) => ({ ...s, ...patch }))
  }

  async function pickLanguage(l: string) {
    setLanguage(l)
    try {
      await persist({ language: l })
    } catch (e) {
      error = errorText(e)
    }
  }

  /**
   * The keychain entry is named after the username, so it is saved before the password. A new
   * username has its own entry (or none), so the saved-password status is read again.
   */
  async function saveUser() {
    const next = username.trim() || 'admin'
    const changed = next !== app.settings?.username
    await persist({ username: next })
    if (changed) await refreshCred()
  }

  /** Empty field: on to step 3 without saving or connecting. Otherwise save and wait for the answer. */
  async function loginNext() {
    error = null
    if (password === '' || slow) {
      busy = true
      try {
        if (!slow) await saveUser()
        step = 3
      } catch (e) {
        error = errorText(e)
      } finally {
        busy = false
      }
      return
    }
    await panel?.submit()
  }

  function onConnected() {
    connected = true
    const reduced = typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches
    const go = () => {
      if (step === 2) step = 3
    }
    if (reduced) go()
    else timers.push(setTimeout(go, OK_PAUSE_MS))
  }

  async function done() {
    busy = true
    error = null
    try {
      await persist({ onboardingDone: true, ...(step === TOTAL ? { updateCheck: allowUpdates } : {}) })
    } catch (e) {
      error = errorText(e)
    } finally {
      busy = false
    }
  }
</script>

<div class="backdrop">
  <div class="card" role="dialog" aria-modal="true" aria-labelledby="onb-title" aria-describedby="onb-step">
    <div class="progress" aria-hidden="true">
      {#each Array.from({ length: TOTAL }, (_, i) => i + 1) as n (n)}<i class:done={n <= step}></i>{/each}
    </div>
    <header>
      <span id="onb-step" class="muted">
        {t('onboarding.label')} · {t('onboarding.step', {
          n: formatNumber(step, { digits }),
          total: formatNumber(TOTAL, { digits }),
        })}
      </span>
      <button type="button" class="link" onclick={done} disabled={busy}>{t('onboarding.skip')}</button>
    </header>

    {#if step === 1}
      <h2 id="onb-title" tabindex="-1" bind:this={heading}>{t('onboarding.welcomeTitle')}</h2>
      <p>{t('onboarding.welcomeBody')}</p>
      <p class="muted">{t('onboarding.welcomeLocal')}</p>
      <div class="langs" role="group" aria-label={t('onboarding.languagePick')}>
        {#each LANGUAGES as l (l.code)}
          <button type="button" lang={l.code} aria-pressed={lang === l.code} onclick={() => pickLanguage(l.code)}
            >{l.name}</button
          >
        {/each}
      </div>
    {:else if step === 2}
      <h2 id="onb-title" tabindex="-1" bind:this={heading}>{t('onboarding.loginTitle')}</h2>
      {#if routerProbe.view?.support === 'unknown'}
        <p class="warn" role="status" data-testid="onb-unsupported">{t('unsupported.onboarding')}</p>
      {/if}
      <LoginPanel
        variant="onboarding"
        bind:this={panel}
        bind:password
        bind:checking
        beforeSave={saveUser}
        onconnected={onConnected}
      />
      {#if editUser}
        <label>
          <span>{t('settings.username')}</span>
          <input type="text" dir="ltr" bind:value={username} spellcheck="false" autocomplete="off" disabled={checking} />
        </label>
      {:else}
        <button type="button" class="link start" onclick={() => (editUser = true)}>{t('login.changeUser')}</button>
      {/if}
      {#if connected}<p class="ok" role="status">{t('onboarding.loginOk')}</p>{/if}
      {#if slow && checking}<p class="muted" role="status">{t('onboarding.loginSlow')}</p>{/if}
    {:else}
      <h2 id="onb-title" tabindex="-1" bind:this={heading}>{t('onboarding.safetyTitle')}</h2>
      <p>{t('onboarding.safetyApply')}</p>
      <ul>
        <li>{t('onboarding.safetyKeep', { keep: t('safeApply.keep') })}</li>
        <li>{t('onboarding.safetyRevert', { revert: t('safeApply.revert') })}</li>
        <li>{t('onboarding.safetyAuto')}</li>
      </ul>
      <p class="warn" role="note">{t('onboarding.safetyWarning')}</p>
      <label class="updates">
        <input type="checkbox" bind:checked={allowUpdates} />
        <span>{t('updates.question')} <span class="muted">{t('updates.questionHint')}</span></span>
      </label>
    {/if}

    {#if error}<p class="err" role="alert">{error}</p>{/if}

    <footer>
      {#if step > 1}
        <button type="button" onclick={() => (step -= 1)} disabled={busy}>{t('onboarding.back')}</button>
      {/if}
      {#if step === 1}
        <button type="button" class="primary" onclick={() => (step = 2)}>{t('onboarding.next')}</button>
      {:else if step === 2}
        {#if (slow && checking) || connected}
          <button type="button" class="primary" onclick={() => (step = 3)}>{t('onboarding.next')}</button>
        {:else}
          <button type="button" class="primary" onclick={loginNext} disabled={busy || checking}>
            {busy ? t('onboarding.saving') : password === '' && !checking ? t('onboarding.loginSkip') : t('login.saveConnect')}
          </button>
        {/if}
      {:else}
        <button type="button" class="primary" onclick={done} disabled={busy}>
          {busy ? t('onboarding.saving') : t('onboarding.finish')}
        </button>
      {/if}
    </footer>
  </div>
</div>

<style>
  .updates {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    margin-block-start: 12px;
    cursor: pointer;
  }
  .updates input {
    margin-block-start: 4px;
    accent-color: var(--accent);
  }
  /* Below the title bar, so the window can still be moved and closed. */
  .backdrop {
    position: fixed;
    inset: 0;
    inset-block-start: var(--titlebar-height);
    display: grid;
    place-items: center;
    padding: 16px;
    background: var(--scrim);
    z-index: 50;
  }
  .card {
    inline-size: min(560px, 100%);
    max-block-size: 100%;
    overflow: auto;
    background: var(--surface);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: var(--radius-card);
    box-shadow: var(--shadow-pop);
    padding-block: 20px;
    padding-inline: 24px;
    display: flex;
    flex-direction: column;
    gap: 12px;
    animation: pop-in var(--t-enter) var(--ease);
  }
  .progress {
    display: flex;
    gap: 4px;
  }
  .progress i {
    flex: 1;
    block-size: 4px;
    border-radius: 2px;
    background: var(--q-track);
  }
  .progress i.done {
    background: var(--primary);
  }
  header,
  footer {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--gap);
  }
  header {
    font-size: var(--fs-small);
  }
  footer {
    justify-content: flex-end;
    margin-block-start: 6px;
  }
  h2 {
    margin: 0;
    font-size: 20px;
    line-height: 1.35;
    outline: none;
  }
  p,
  ul {
    margin: 0;
  }
  ul {
    padding-inline-start: 1.2em;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .ok {
    background: var(--pill-positive-bg);
    color: var(--pill-positive-ink);
    border-radius: var(--radius-control);
    padding-block: 8px;
    padding-inline: 12px;
  }
  .start {
    align-self: flex-start;
  }
  label {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-weight: 500;
  }
  input {
    font-weight: 400;
  }
  .langs {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  .langs [aria-pressed='true'] {
    border-color: var(--text);
    background: var(--active);
    font-weight: 500;
  }
  .muted {
    color: var(--muted);
  }
  .warn {
    background: var(--pill-warning-bg);
    color: var(--pill-warning-ink);
    border-radius: var(--radius-control);
    padding-block: 10px;
    padding-inline: 12px;
    font-size: var(--fs-small);
  }
  .err {
    color: var(--q-poor);
  }
  .link {
    background: none;
    border: none;
    color: var(--muted);
    text-decoration: underline;
    cursor: pointer;
    font: inherit;
    padding: 0;
  }
  .link:hover {
    color: var(--text);
  }
</style>
