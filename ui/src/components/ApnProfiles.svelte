<script lang="ts">
  // The router's APN profiles: which one is in use, which ones the operator set, "Use this APN"
  // for the others through safe apply, and add, edit and delete for profiles that aren't in use.
  import CircleCheck from '@lucide/svelte/icons/circle-check'
  import Info from '@lucide/svelte/icons/info'
  import Lock from '@lucide/svelte/icons/lock'
  import Pencil from '@lucide/svelte/icons/pencil'
  import Plus from '@lucide/svelte/icons/plus'
  import Trash2 from '@lucide/svelte/icons/trash-2'
  import RefreshCw from '@lucide/svelte/icons/refresh-cw'
  import Waypoints from '@lucide/svelte/icons/waypoints'
  import { api } from '../lib/backend'
  import { app, pushToast } from '../lib/store.svelte'
  import { t, type MessageKey } from '../lib/i18n'
  import { errorText } from '../lib/errors'
  import { apnEditRefusal, apnSwitchRefusal, profileEditRefusal, profileOnlyEditRefusal, profileRefusal } from '../lib/apn'
  import { scanActive } from '../lib/scan.svelte'
  import { speed } from '../lib/speedtest.svelte'
  import type { ApnIpType } from '../lib/bindings/ApnIpType'
  import type { ApnProfile } from '../lib/bindings/ApnProfile'
  import ApnSwitchDialog from './ApnSwitchDialog.svelte'
  import ApnEditDialog from './ApnEditDialog.svelte'

  const IP_TYPE: Record<ApnIpType, MessageKey> = {
    ipv4: 'network.apn.ipType.ipv4',
    ipv6: 'network.apn.ipType.ipv6',
    ipv4v6: 'network.apn.ipType.ipv4v6',
    other: 'network.apn.ipType.other',
  }

  const snap = $derived(app.snapshot)
  const loggedIn = $derived(snap?.session.kind === 'loggedIn')
  const support = $derived(snap?.capabilities?.apn ?? null)
  const list = $derived(loggedIn ? (snap?.apn ?? null) : null)
  let refreshing = $state(false)
  const canSwitch = $derived(loggedIn && support?.switch === true)
  const refusal = $derived(canSwitch ? apnSwitchRefusal(snap, { scan: scanActive(), speedTest: speed.running }) : null)
  let choosing: ApnProfile | null = $state(null)
  const canEdit = $derived(loggedIn && support?.edit === true)
  const editRefusal = $derived(canEdit ? apnEditRefusal(snap, { scan: scanActive(), speedTest: speed.running }) : null)
  let editingWhat: { mode: 'add' | 'edit' | 'delete'; profile: ApnProfile | null } | null = $state(null)
  const current = $derived(list?.profiles.find((p) => p.index === list?.current) ?? null)

  async function refresh() {
    if (refreshing) return
    refreshing = true
    try {
      await api.apnRefresh()
    } catch (e) {
      pushToast(errorText(e), 'error')
    } finally {
      refreshing = false
    }
  }
</script>

{#if !loggedIn || support?.read}
  <section class="card" aria-labelledby="apn-title" data-testid="apn-card">
    <div class="head">
      <h2 id="apn-title"><Waypoints size={18} aria-hidden="true" />{t('network.apn.title')}</h2>
      {#if loggedIn}
        <div class="head-actions">
          {#if canEdit && list}
            <button
              type="button"
              class="secondary"
              disabled={editRefusal !== null}
              title={editRefusal ? t(editRefusal) : undefined}
              onclick={() => (editingWhat = { mode: 'add', profile: null })}
              data-testid="apn-add"><Plus size={14} aria-hidden="true" />{t('network.apn.edit.add')}</button
            >
          {/if}
          <button type="button" class="secondary" disabled={refreshing} onclick={refresh} data-testid="apn-refresh"
            ><RefreshCw size={14} aria-hidden="true" />{t('network.apn.refresh')}</button
          >
        </div>
      {/if}
    </div>
    <p class="note">{t('network.apn.explainer')}</p>
    {#if !loggedIn}
      <p class="note" data-testid="apn-unknown">{t('network.apn.loggedOut')}</p>
    {:else if !list}
      <p class="note" data-testid="apn-unknown">{t('network.apn.unknown')}</p>
    {:else}
      {#if list.autoApn === true}
        <p class="callout" data-testid="apn-auto">{t('network.apn.autoOn')}</p>
      {:else if refusal}
        <p class="note small" role="status" data-testid="apn-why">{t(refusal)}</p>
      {/if}
      {#if list.profiles.length === 0}
        <p class="note">{t('network.apn.empty')}</p>
      {:else}
        <ul class="rows" aria-label={t('network.apn.listLabel')}>
          {#each list.profiles as p (p.index)}
            {@const inUse = list.current === p.index}
            <li class:current={inUse} data-testid="apn-row">
              <div class="main">
                <span class="name"><bdi>{p.name || t('network.apn.unnamed')}</bdi></span>
                <span class="apn" data-testid="apn-value"
                  >{#if p.apn}<bdi dir="ltr">{p.apn}</bdi>{:else}{t('network.apn.byNetwork')}{/if}</span
                >
              </div>
              <div class="tags">
                <span class="tag">{t(IP_TYPE[p.ipType])}</span>
                {#if p.readOnly}
                  <span class="tag" data-testid="apn-operator"><Lock size={12} aria-hidden="true" />{t('network.apn.operator')}</span>
                {/if}
                {#if !p.valid}
                  <span class="tag" data-testid="apn-invalid">{t('network.apn.invalid')}</span>
                {/if}
                {#if inUse}
                  <span class="tag in-use" data-testid="apn-in-use"
                    ><CircleCheck size={12} aria-hidden="true" />{t('network.apn.inUse')}</span
                  >
                {/if}
                {#if canSwitch && !profileRefusal(p, list.current)}
                  <button
                    type="button"
                    class="use"
                    disabled={refusal !== null}
                    title={refusal ? t(refusal) : undefined}
                    onclick={() => (choosing = p)}
                    data-testid="apn-use">{t('network.apn.use')}</button
                  >
                {/if}
                {#if canEdit}
                  {@const why = profileEditRefusal(p, list.current)}
                  {#if why}
                    <span class="locked" role="img" title={t(why)} aria-label={t(why)} data-testid="apn-edit-why"
                      ><Info size={14} aria-hidden="true" /></span
                    >
                  {:else}
                    {@const editOnly = profileOnlyEditRefusal(p)}
                    {#if editOnly}
                      <span class="locked" role="img" title={t(editOnly)} aria-label={t(editOnly)} data-testid="apn-edit-why"
                        ><Info size={14} aria-hidden="true" /></span
                      >
                    {:else}
                      <button
                        type="button"
                        class="icon"
                        disabled={editRefusal !== null}
                        title={editRefusal ? t(editRefusal) : t('network.apn.edit.editButton')}
                        aria-label={t('network.apn.edit.editButton')}
                        onclick={() => (editingWhat = { mode: 'edit', profile: p })}
                        data-testid="apn-edit"><Pencil size={14} aria-hidden="true" /></button
                      >
                    {/if}
                    <button
                      type="button"
                      class="icon"
                      disabled={editRefusal !== null}
                      title={editRefusal ? t(editRefusal) : t('network.apn.edit.deleteButton')}
                      aria-label={t('network.apn.edit.deleteButton')}
                      onclick={() => (editingWhat = { mode: 'delete', profile: p })}
                      data-testid="apn-delete"><Trash2 size={14} aria-hidden="true" /></button
                    >
                  {/if}
                {/if}
              </div>
            </li>
          {/each}
        </ul>
      {/if}
      {#if support && !support.confirmed}
        <p class="note small" data-testid="apn-unconfirmed">{t('network.apn.unconfirmed')}</p>
      {/if}
    {/if}
  </section>
{/if}

{#if choosing}
  <ApnSwitchDialog profile={choosing} {current} onclose={() => (choosing = null)} />
{/if}

{#if editingWhat}
  <ApnEditDialog mode={editingWhat.mode} profile={editingWhat.profile} {current} onclose={() => (editingWhat = null)} />
{/if}

<style>
  .card {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius-card);
    padding-block: var(--card-pad-block);
    padding-inline: var(--card-pad-inline);
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-inline-size: 0;
  }
  .head {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }
  h2 {
    margin: 0;
  }
  .note {
    margin: 0;
    color: var(--muted);
    max-inline-size: var(--measure);
  }
  .small {
    font-size: var(--fs-small);
  }
  .callout {
    margin: 0;
    padding-block: 8px;
    padding-inline: 12px;
    border: 1px solid var(--border);
    border-inline-start: 3px solid var(--accent);
    border-radius: var(--radius-control);
    background: var(--raised);
  }
  .rows {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .rows li {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 6px 12px;
    padding-block: 8px;
    padding-inline: 12px;
    border: 1px solid var(--border);
    border-radius: var(--radius-control);
  }
  .rows li.current {
    border-color: var(--accent);
  }
  .main {
    display: flex;
    flex-direction: column;
    min-inline-size: 0;
  }
  .name {
    font-weight: 500;
    overflow-wrap: anywhere;
  }
  .apn {
    color: var(--muted);
    font-size: var(--fs-small);
    overflow-wrap: anywhere;
  }
  .tags {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .tag {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding-block: 2px;
    padding-inline: 8px;
    border: 1px solid var(--border);
    border-radius: var(--radius-control);
    background: var(--raised);
    font-size: var(--fs-small);
  }
  .tag.in-use {
    font-weight: 500;
    border-color: var(--accent);
  }
  .use {
    min-block-size: 28px;
    padding-block: 2px;
    padding-inline: 10px;
    font-size: var(--fs-small);
  }
  .head-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  .icon {
    display: inline-grid;
    place-items: center;
    min-block-size: 28px;
    min-inline-size: 28px;
    padding: 0;
  }
  .locked {
    display: inline-grid;
    place-items: center;
    min-block-size: 28px;
    min-inline-size: 28px;
    color: var(--muted);
    cursor: help;
  }
  .head button {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    min-block-size: var(--control-height);
    padding-inline: 10px;
  }
</style>
