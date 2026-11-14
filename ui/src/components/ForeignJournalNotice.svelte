<script lang="ts">
  import { app, pushToast, updateSettings } from '../lib/store.svelte'
  import { t } from '../lib/i18n'
  import { isolate } from '../lib/format'
  import { errorText } from '../lib/errors'
  import CircleAlert from '@lucide/svelte/icons/circle-alert'
  import type { DriverId } from '../lib/bindings/DriverId'
  import type { Settings } from '../lib/bindings/Settings'

  const journal = $derived(app.snapshot?.foreignJournal ?? null)
  const known = (d: string): d is DriverId => d === 'huawei_h122' || d === 'mock'
  const driverName = $derived(
    journal && known(journal.driver)
      ? journal.driver === 'mock'
        ? t('settings.driverMock')
        : t('settings.driverHuawei')
      : (journal?.driver ?? ''),
  )
  let busy = $state(false)

  async function switchBack() {
    if (!journal || !app.settings || !known(journal.driver) || busy) return
    busy = true
    try {
      const host = journal.host
      const driver: DriverId = journal.driver
      await updateSettings((s): Settings => ({ ...s, host, driver }))
    } catch (e) {
      pushToast(errorText(e), 'error')
    } finally {
      busy = false
    }
  }
</script>

{#if journal}
  <section class="notice" role="alert" aria-labelledby="foreign-title">
    <h2 id="foreign-title"><CircleAlert size={18} aria-hidden="true" />{t('foreign.title')}</h2>
    <p>
      {t('foreign.body', { driver: isolate(driverName), host: isolate(journal.host) })}
    </p>
    <p class="muted">{t('safeApply.foreignJournal')}</p>
    {#if !known(journal.driver)}<p class="muted">{t('foreign.unknownDriver')}</p>{/if}
    <div class="row">
      <button type="button" class="primary" disabled={busy || !known(journal.driver)} onclick={switchBack}
        >{t('foreign.switchBack')}</button
      >
    </div>
    <p class="muted">{t('foreign.discardHelp')}</p>
  </section>
{/if}

<style>
  .notice {
    border: 1px solid var(--border);
    border-inline-start: 4px solid var(--q-poor);
    border-radius: var(--radius-card);
    background: var(--surface);
    padding-block: 14px;
    padding-inline: 18px;
  }
  h2 {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: var(--fs-section);
    margin-block: 0 6px;
  }
  h2 :global(svg) {
    flex: none;
    color: var(--q-poor);
  }
  p {
    margin-block: 0 8px;
  }
  .muted {
    color: var(--muted);
    font-size: var(--fs-small);
  }
  .row {
    margin-block: 4px 10px;
  }
</style>
