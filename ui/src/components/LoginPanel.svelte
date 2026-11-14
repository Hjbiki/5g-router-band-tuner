<script module lang="ts">
  // "Not now" on the router question hides the app-wide banner until the question goes away.
  // Settings still shows it, and the sidebar still names it.
  const dismissed = $state({ confirm: false })

  /** For tests: forget a "Not now". */
  export function resetLoginPanel(): void {
    dismissed.confirm = false
  }
</script>

<script lang="ts">
  import { onMount, untrack } from 'svelte'
  import KeyRound from '@lucide/svelte/icons/key-round'
  import ShieldQuestion from '@lucide/svelte/icons/shield-question-mark'
  import LoaderCircle from '@lucide/svelte/icons/loader-circle'
  import CircleCheck from '@lucide/svelte/icons/circle-check'
  import CircleAlert from '@lucide/svelte/icons/circle-alert'
  import Eye from '@lucide/svelte/icons/eye'
  import EyeOff from '@lucide/svelte/icons/eye-off'
  import Plug from '@lucide/svelte/icons/plug'
  import RefreshCw from '@lucide/svelte/icons/refresh-cw'
  import LogOut from '@lucide/svelte/icons/log-out'
  import InfoTip from './ui/InfoTip.svelte'
  import { api } from '../lib/backend'
  import { app, pushToast } from '../lib/store.svelte'
  import { t, type MessageKey } from '../lib/i18n'
  import { errorText } from '../lib/errors'
  import { formatNumber, isolate } from '../lib/format'
  import { loginUptime } from '../lib/uptime'
  import { credentials, refreshCred } from '../lib/credentials.svelte'
  import { loginState, needsAttention, onboardingOpen, type LoginState } from '../lib/login'
  import { changeOpen } from '../lib/bands'
  import { apnChangeOpen } from '../lib/apn'
  import { ipRefreshActive } from '../lib/ipRefresh.svelte'
  import { scanActive } from '../lib/scan.svelte'
  import { speed } from '../lib/speedtest.svelte'

  type Variant = 'banner' | 'settings' | 'onboarding'
  let {
    variant,
    accountDirty = false,
    onconnected,
    beforeSave,
    password = $bindable(''),
    checking = $bindable(false),
  }: {
    variant: Variant
    /** Settings: the address, username or driver is edited but not saved yet. */
    accountDirty?: boolean
    /** Onboarding: the password typed here logged in. */
    onconnected?: () => void
    /** Runs before the password is saved (onboarding saves the username first). */
    beforeSave?: () => Promise<void>
    password?: string
    checking?: boolean
  } = $props()

  /** A launch connect starts within this long; until then a saved password isn't "not connected". */
  const LAUNCH_HOLD_MS = 2000
  /** Gives up waiting for the login's answer after this long; the panel then shows the state. */
  const CHECK_CAP_MS = 30_000
  /** The H122 ends a session 5 minutes after its last login step, so an unconfirmed logout ends by then. */
  const SESSION_EXPIRY_MIN = 5

  const uid = $props.id()
  const inputId = `${uid}-pw`
  const bodyId = `${uid}-body`
  const logoutNoteId = `${uid}-logout-note`

  const lstate: LoginState = $derived(loginState(app.snapshot?.session ?? null, credentials.status))
  const digits = $derived(app.settings?.digits ?? 'western')
  const host = $derived(app.settings?.host ?? '')
  const account = $derived(isolate(`${app.settings?.host ?? ''}/${app.settings?.username ?? ''}`))
  const demo = $derived(app.settings?.driver === 'mock')

  let show = $state(false)
  let caps = $state(false)
  let submitting = $state(false)
  let error: string | null = $state(null)
  /** Settings: "Change password" opened the form in a state that doesn't show it by itself. */
  let changing = $state(false)
  let confirmingRemove = $state(false)
  let submitted = $state(false)
  // Only the banner holds back a saved-but-not-connected state at launch.
  let settled = $state(untrack(() => variant) !== 'banner')
  let input: HTMLInputElement | undefined = $state()
  let removeCancel: HTMLButtonElement | undefined = $state()

  let now = $state(Date.now())
  onMount(() => {
    const id = setInterval(() => (now = Date.now()), 1000)
    const hold = settled ? undefined : setTimeout(() => (settled = true), LAUNCH_HOLD_MS)
    return () => {
      clearInterval(id)
      if (hold) clearTimeout(hold)
    }
  })

  // The lockout countdown starts when the state first appears.
  const locked = $derived(lstate.kind === 'locked')
  let lockUntil: number | null = $state(null)
  $effect(() => {
    if (!locked) {
      lockUntil = null
      return
    }
    untrack(() => {
      if (lockUntil === null && lstate.kind === 'locked') {
        lockUntil = Date.now() + lstate.waitS * 1000
        now = Date.now()
      }
    })
  })
  const lockLeft = $derived(lockUntil === null ? 0 : Math.max(0, Math.ceil((lockUntil - now) / 1000)))
  const lockText = $derived.by(() => {
    const m = Math.floor(lockLeft / 60)
    const s = lockLeft % 60
    const two = (n: number) => formatNumber(Math.floor(n / 10), { digits }) + formatNumber(n % 10, { digits })
    return isolate(`${formatNumber(m, { digits })}:${two(s)}`)
  })

  // Checking lasts from the submit until the login has been tried and answered. Submitted while
  // logged in ("Change password"), the old session isn't the answer: only one after a working state.
  let sawWorking = false
  let wasConnected = false
  let checkCap: ReturnType<typeof setTimeout> | undefined
  function stopChecking() {
    checking = false
    if (checkCap) clearTimeout(checkCap)
    checkCap = undefined
  }
  $effect(() => {
    if (!checking) return
    const k = lstate.kind
    if (k === 'working') sawWorking = true
    else if ((k === 'connected' && !wasConnected) || (sawWorking && k !== 'loading')) untrack(stopChecking)
  })
  $effect(() => () => {
    if (checkCap) clearTimeout(checkCap)
  })

  $effect(() => {
    if (submitted && lstate.kind === 'connected' && !checking) {
      untrack(() => {
        submitted = false
        changing = false
        onconnected?.()
      })
    }
  })

  $effect(() => {
    if (lstate.kind !== 'confirmRouter' && lstate.kind !== 'loading') dismissed.confirm = false
  })
  $effect(() => {
    if (confirmingRemove) removeCancel?.focus()
  })
  const visible = $derived.by(() => {
    if (variant !== 'banner') return true
    // Settings shows the same state in its Router connection panel; the guide asks itself.
    if (app.tab === 'settings' || onboardingOpen(app.settings, app.snapshot)) return false
    if (!needsAttention(lstate)) return false
    if (lstate.kind === 'confirmRouter' && dismissed.confirm) return false
    if (lstate.kind === 'ready' && lstate.why === 'saved' && !settled) return false
    return true
  })

  // A wrong password in the banner puts the cursor straight in the field.
  $effect(() => {
    if (variant === 'banner' && visible && lstate.kind === 'wrongPassword') untrack(() => input?.focus())
  })

  /** The states whose only way forward is typing a password. */
  const asksPassword = $derived(
    lstate.kind === 'needPassword' ||
      lstate.kind === 'wrongPassword' ||
      lstate.kind === 'locked' ||
      // Logged out, then the saved password was removed: Connect has nothing to send.
      (lstate.kind === 'loggedOut' && credentials.status?.saved === false),
  )
  const showForm = $derived(
    !demo &&
      lstate.kind !== 'unsupported' &&
      (asksPassword ||
        checking ||
        (changing && (lstate.kind === 'connected' || lstate.kind === 'ready' || lstate.kind === 'loggedOut')) ||
        (variant === 'onboarding' && lstate.kind !== 'connected')),
  )

  /** Why Log out waits: the backend refuses it while anything else holds the router. */
  const logoutBlock = $derived.by((): MessageKey | null => {
    if (changeOpen(app.snapshot?.safeApply)) return 'login.logoutBlocked'
    if (ipRefreshActive(app.snapshot)) return 'login.logoutBlockedIpRefresh'
    if (apnChangeOpen(app.snapshot)) return 'login.logoutBlockedApn'
    if (scanActive()) return 'login.logoutBlockedScan'
    if (speed.running) return 'login.logoutBlockedTest'
    return null
  })
  let loggingOut = $state(false)
  const fieldDisabled = $derived(checking || submitting || accountDirty || (locked && lockLeft > 0))
  const canSubmit = $derived(!fieldDisabled && password !== '')

  const tone = $derived.by((): 'neutral' | 'warn' | 'bad' | 'ok' => {
    switch (lstate.kind) {
      case 'connected':
        return 'ok'
      case 'working':
      case 'confirmRouter':
        return 'warn'
      case 'wrongPassword':
      case 'locked':
      case 'unsupported':
        return 'bad'
      default:
        return 'neutral'
    }
  })

  const title = $derived.by((): string => {
    switch (lstate.kind) {
      case 'loading':
        return t('app.loading')
      case 'connected':
        return t('settings.connectedToRouter')
      case 'working':
        return t('login.title.working')
      case 'needPassword':
        return t('login.title.needPassword')
      case 'wrongPassword':
        return t('login.title.wrong')
      case 'locked':
        return t('login.title.locked')
      case 'confirmRouter':
        return t('login.title.confirm')
      case 'ready':
        return t('login.title.ready')
      case 'unsupported':
        return t('login.title.unsupported')
      case 'loggedOut':
        return t('login.title.loggedOut')
    }
  })

  const body = $derived.by((): string | null => {
    const s = lstate
    switch (s.kind) {
      case 'loading':
        return null
      case 'connected':
        return demo ? t('login.demo') : credentials.status?.saved ? t('login.savedFor', { account }) : null
      case 'working':
        return s.why === 'connecting'
          ? t('login.body.connecting')
          : s.why === 'retrying'
            ? t('login.body.retrying', { seconds: isolate(formatNumber(s.retryInS ?? null, { digits })) })
            : t('login.body.takenOver')
      case 'needPassword':
        return t('login.body.needPassword', { host: isolate(host) })
      case 'wrongPassword':
        return s.code === undefined
          ? t('login.body.wrong')
          : t('login.body.rejected', { code: isolate(formatNumber(s.code, { digits })) })
      case 'locked':
        return lockLeft > 0 ? t('login.body.locked', { time: lockText }) : t('login.body.lockedOver')
      case 'confirmRouter':
        return t(s.firstTime ? 'login.body.confirmNew' : 'login.body.confirmChanged', { host: isolate(host) })
      case 'ready':
        return t(s.why === 'token' ? 'login.body.token' : 'login.body.ready')
      case 'unsupported':
        return t('login.body.unsupported')
      case 'loggedOut':
        return t('login.body.loggedOut')
    }
  })

  const uptime = $derived(loginUptime(lstate.kind === 'connected', now, digits))

  /** Saves the typed password, then logs in with it. The field is cleared at once. */
  export async function submit(): Promise<void> {
    if (!canSubmit) return
    const pw = password
    password = ''
    show = false
    error = null
    submitting = true
    try {
      await beforeSave?.()
      // Saving clears the rejected-password marker, for this new password only.
      await api.saveCredentials(pw)
      await refreshCred()
      sawWorking = false
      wasConnected = lstate.kind === 'connected'
      checking = true
      submitted = true
      checkCap = setTimeout(stopChecking, CHECK_CAP_MS)
      await api.connect()
    } catch (e) {
      error = errorText(e)
      stopChecking()
      submitted = false
    } finally {
      submitting = false
    }
  }

  async function run(f: () => Promise<void>) {
    error = null
    try {
      await f()
    } catch (e) {
      error = errorText(e)
    }
  }

  const connect = () => run(() => api.connect())

  /** One click, no question: nothing is lost, and Connect brings it all back. */
  async function logout() {
    if (logoutBlock || loggingOut) return
    loggingOut = true
    await run(async () => {
      const r = await api.logout()
      changing = false
      pushToast(t('login.loggedOutToast'))
      if (!r.confirmed)
        pushToast(t('login.logoutUnconfirmed', { minutes: isolate(formatNumber(SESSION_EXPIRY_MIN, { digits })) }))
    })
    loggingOut = false
  }
  const trust = () => run(() => api.connectTrusting())

  async function remove() {
    await run(async () => {
      await api.forgetCredentials()
      await refreshCred()
      pushToast(t('login.removed'))
    })
    confirmingRemove = false
    changing = false
  }

  function onKey(e: KeyboardEvent) {
    caps = e.getModifierState?.('CapsLock') ?? false
    if (e.type === 'keydown' && e.key === 'Enter') {
      // Never the surrounding Settings form.
      e.preventDefault()
      void submit()
    }
  }
</script>

{#snippet passwordForm()}
  <div class="pw">
    <div class="pw-row">
      <div class="field">
        <input
          id={inputId}
          bind:this={input}
          type={show ? 'text' : 'password'}
          dir="ltr"
          bind:value={password}
          autocomplete="new-password"
          spellcheck="false"
          aria-label={t('settings.password')}
          aria-describedby={body ? bodyId : undefined}
          disabled={fieldDisabled}
          onkeydown={onKey}
          onkeyup={onKey}
        />
        <button
          type="button"
          class="eye"
          aria-pressed={show}
          aria-label={show ? t('login.hide') : t('login.show')}
          aria-controls={inputId}
          onclick={() => (show = !show)}
          >{#if show}<EyeOff size={16} aria-hidden="true" />{:else}<Eye size={16} aria-hidden="true" />{/if}</button
        >
      </div>
      {#if variant !== 'onboarding'}
        <button type="button" class="primary" disabled={!canSubmit} onclick={() => void submit()}
          >{t('login.saveConnect')}</button
        >
      {/if}
    </div>
    {#if caps && !fieldDisabled}<p class="caps" role="status">{t('login.capsLock')}</p>{/if}
    {#if accountDirty}<p class="note" data-testid="login-account-dirty">{t('login.accountDirty')}</p>{/if}
    {#if checking}
      <p class="checking" role="status">
        <span class="spin" aria-hidden="true"><LoaderCircle size={14} /></span>{t('login.checking')}
      </p>
    {/if}
    <p class="stored">
      {t('login.stored')}
      <InfoTip label={t('login.whereSaved')} text={t('login.whereSavedHelp', { account })} />
    </p>
  </div>
{/snippet}

{#snippet actions()}
  {#if lstate.kind === 'confirmRouter'}
    <div class="actions">
      <button type="button" class="primary" onclick={trust}>{t('login.confirmYes')}</button>
      {#if variant === 'banner'}
        <button type="button" onclick={() => (dismissed.confirm = true)}>{t('login.notNow')}</button>
      {/if}
    </div>
  {:else if lstate.kind === 'ready' && !checking && !demo}
    <div class="actions">
      {#if lstate.why === 'token'}
        <button type="button" class="primary" onclick={connect}>{t('login.tryAgain')}</button>
      {:else}
        <button type="button" class="primary" onclick={connect}
          ><Plug size={16} aria-hidden="true" />{t('settings.connect')}</button
        >
        {#if variant !== 'onboarding'}
          <button type="button" aria-expanded={changing} onclick={() => (changing = !changing)}>{t('login.change')}</button>
        {/if}
      {/if}
    </div>
  {:else if lstate.kind === 'ready' && demo}
    <div class="actions">
      <button type="button" class="primary" onclick={connect}
        ><Plug size={16} aria-hidden="true" />{t('settings.connect')}</button
      >
    </div>
  {:else if lstate.kind === 'loggedOut' && !checking}
    <div class="actions">
      {#if !asksPassword}
        <button type="button" class="primary" onclick={connect}
          ><Plug size={16} aria-hidden="true" />{t('settings.connect')}</button
        >
      {/if}
      {#if variant === 'settings' && !demo && credentials.status?.saved}
        <button type="button" aria-expanded={changing} onclick={() => (changing = !changing)}>{t('login.change')}</button>
        <button type="button" onclick={() => (confirmingRemove = true)}>{t('login.remove')}</button>
      {/if}
    </div>
  {:else if lstate.kind === 'connected' && variant === 'settings'}
    <div class="actions">
      {#if !demo}
        <button type="button" aria-expanded={changing} onclick={() => (changing = !changing)}>{t('login.change')}</button>
        {#if credentials.status?.saved}
          <button type="button" onclick={() => (confirmingRemove = true)}>{t('login.remove')}</button>
        {/if}
      {/if}
      <button
        type="button"
        disabled={!!logoutBlock || loggingOut}
        aria-describedby={logoutBlock ? logoutNoteId : undefined}
        onclick={logout}><LogOut size={16} aria-hidden="true" />{t('login.logout')}</button
      >
      <button type="button" class="text" onclick={connect}
        ><RefreshCw size={14} aria-hidden="true" />{t('settings.reconnect')}</button
      >
    </div>
    {#if logoutBlock}<p class="note" id={logoutNoteId}>{t(logoutBlock)}</p>{/if}
  {/if}
{/snippet}

{#if variant === 'onboarding'}
  <div class="onb" data-testid="login-panel">
    {#if demo}
      <p>{t('login.demo')}</p>
    {:else}
      <p id={bodyId}>{t('login.body.needPassword', { host: isolate(host) })}</p>
    {/if}
    {#if lstate.kind === 'wrongPassword' || lstate.kind === 'locked' || lstate.kind === 'unsupported'}
      <div class="inline bad" role="alert">
        <strong>{title}</strong>
        {#if body}<span>{body}</span>{/if}
      </div>
    {:else if lstate.kind === 'confirmRouter'}
      <div class="inline warn" role="status">
        <strong>{title}</strong>
        {#if body}<span>{body}</span>{/if}
        {@render actions()}
      </div>
    {/if}
    {#if showForm}{@render passwordForm()}{/if}
    {#if error}<p class="err" role="alert">{error}</p>{/if}
  </div>
{:else if visible}
  <div
    class="panel {tone}"
    class:banner={variant === 'banner'}
    role={tone === 'bad' ? 'alert' : 'status'}
    data-testid="login-panel"
    data-state={lstate.kind}
  >
    <span class="icon" aria-hidden="true">
      {#if lstate.kind === 'working' || lstate.kind === 'loading'}
        <span class="spin"><LoaderCircle size={18} /></span>
      {:else if lstate.kind === 'connected'}
        <CircleCheck size={18} />
      {:else if lstate.kind === 'confirmRouter'}
        <ShieldQuestion size={18} />
      {:else if lstate.kind === 'loggedOut'}
        <LogOut size={18} />
      {:else if tone === 'bad'}
        <CircleAlert size={18} />
      {:else}
        <KeyRound size={18} />
      {/if}
    </span>
    <div class="main">
      <div class="text">
        <strong
          >{title}{#if uptime && lstate.kind === 'connected'}<span class="uptime num"
              >{t('shell.loggedInFor', { time: isolate(uptime) })}</span
            >{/if}</strong
        >
        {#if body}<p id={bodyId}>{body}</p>{/if}
        {#if demo && variant === 'settings' && lstate.kind !== 'connected'}<p>{t('login.demo')}</p>{/if}
      </div>
      {@render actions()}
      {#if confirmingRemove}
        <div class="remove" role="alertdialog" aria-labelledby="{uid}-rm">
          <p id="{uid}-rm">{t('login.removeConfirm')}</p>
          <div class="actions">
            <button type="button" bind:this={removeCancel} onclick={() => (confirmingRemove = false)}>{t('app.cancel')}</button>
            <button type="button" class="danger" onclick={remove}>{t('login.removeYes')}</button>
          </div>
        </div>
      {/if}
      {#if showForm}{@render passwordForm()}{/if}
      {#if error}<p class="err" role="alert">{error}</p>{/if}
    </div>
  </div>
{/if}

<style>
  .panel {
    display: flex;
    flex-wrap: wrap;
    align-items: flex-start;
    gap: 12px;
    border-radius: var(--radius-card);
    padding-block: 14px;
    padding-inline: 16px;
    border: 1px solid transparent;
  }
  .neutral {
    background: var(--surface);
    border-color: var(--border);
    color: var(--text);
  }
  .warn {
    background: var(--pill-warning-bg);
    color: var(--pill-warning-ink);
  }
  .bad {
    background: var(--pill-negative-bg);
    color: var(--pill-negative-ink);
  }
  .ok {
    background: var(--pill-positive-bg);
    color: var(--pill-positive-ink);
  }
  .icon {
    display: inline-grid;
    flex: none;
    margin-block-start: 1px;
  }
  .main {
    flex: 1;
    min-inline-size: min(16em, 100%);
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .text strong {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 2px 10px;
    font-size: var(--fs-body);
    font-weight: 600;
  }
  .uptime {
    font-size: var(--fs-small);
    font-weight: 400;
    opacity: 0.85;
  }
  .text p,
  .onb p,
  .pw p {
    margin: 0;
  }
  .text p {
    margin-block-start: 2px;
    font-size: var(--fs-small);
    opacity: 0.85;
  }
  .neutral .text p,
  .stored,
  .note {
    color: var(--muted);
    opacity: 1;
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
  }
  .actions button {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  /* On the Router page the buttons share the card's width in even columns, so long labels (Arabic)
     wrap inside their own button instead of leaving a ragged row. */
  .panel:not(.banner) .actions {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(140px, 1fr));
    inline-size: 100%;
  }
  .panel:not(.banner) .actions button {
    justify-content: center;
    min-block-size: var(--control-height);
    block-size: auto;
    padding-block: 6px;
    white-space: normal;
    text-align: center;
  }
  .text {
    text-align: start;
  }
  button.text {
    background: none;
    border-color: transparent;
    color: inherit;
    text-decoration: underline;
    padding-inline: 4px;
  }
  .pw {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .pw-row {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  .field {
    flex: 1 1 14em;
    display: flex;
    position: relative;
    min-inline-size: 0;
  }
  .field input {
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
    background: none;
    border-color: transparent;
    color: var(--muted);
  }
  .pw-row > .primary {
    flex: none;
  }
  .caps,
  .note,
  .stored,
  .checking {
    font-size: var(--fs-small);
  }
  .main > .note {
    margin: 0;
  }
  .caps {
    color: var(--pill-warning-ink);
  }
  .stored {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 4px;
  }
  .checking {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .spin {
    display: inline-grid;
    animation: login-spin 1s linear infinite;
  }
  @keyframes login-spin {
    to {
      transform: rotate(360deg);
    }
  }
  .remove {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding-block: 10px;
    padding-inline: 12px;
    border-radius: var(--radius-control);
    border: 1px solid var(--danger-border);
    background: var(--surface);
    color: var(--text);
  }
  .err {
    color: var(--q-poor);
    font-size: var(--fs-small);
  }
  .onb {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .inline {
    display: flex;
    flex-direction: column;
    gap: 6px;
    border-radius: var(--radius-control);
    padding-block: 10px;
    padding-inline: 12px;
    font-size: var(--fs-small);
  }
  @media (prefers-reduced-motion: reduce) {
    .spin {
      animation: none;
    }
  }
</style>
