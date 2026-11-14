<script lang="ts">
  // The History page's answer, first: which bands have been fastest here, in a sentence, with the
  // one thing to do next. The rules are in historyAnswer.ts. "Try these bands" only fills the band
  // editor; nothing reaches the router until the user applies it on the Bands page.
  import Trophy from '@lucide/svelte/icons/trophy'
  import { app, canTryBands, tryBands } from '../../lib/store.svelte'
  import { t } from '../../lib/i18n'
  import { formatNumber, isolate } from '../../lib/format'
  import { mbps } from '../../lib/speedtest.svelte'
  import { supportedBands } from '../../lib/bands'
  import { bandsText, history, type BandSetRow } from '../../lib/history.svelte'
  import { ANSWER_MIN_TESTS, CLEAR_LEAD, answerFor } from '../../lib/historyAnswer'

  const digits = $derived(app.settings?.digits ?? 'western')
  const num = (n: number) => formatNumber(n, { digits })
  const pct = (n: number) => t('history.cards.percent', { n: num(n) })
  const speed = (v: number | null) => t('speedtest.unit.speed', { n: mbps(v, digits) })
  const label = (r: BandSetRow) =>
    r.served !== null ? t('history.bands.served', { band: isolate(`B${r.served}`) }) : isolate(bandsText(supportedBands(r.bands)))

  const answer = $derived(answerFor(history.speeds))
  const basis = $derived(
    answer.kind === 'noTests' ? null : t('history.answer.basis', { provider: t(`speedtest.provider.${answer.provider}`), n: num(answer.tests) }),
  )
  // One band set has nothing to switch to: its next step is a scan, not "try these bands".
  const tryRow = $derived(answer.kind === 'fastest' ? answer.row : null)
  const canTry = $derived(canTryBands())

  function seeTable() {
    app.historyFocus = 'bandSets'
  }
</script>

<section class="answer" aria-labelledby="history-answer" class:stale={history.loading} data-testid="history-answer" data-kind={answer.kind}>
  <h2 id="history-answer" tabindex="-1"><Trophy size={18} aria-hidden="true" />{t('history.answer.title')}</h2>

  {#if answer.kind === 'noTests'}
    <p class="main">{t('history.answer.noTests')}</p>
    <p class="muted">{t('history.answer.noTestsHint')}</p>
  {:else if answer.kind === 'fewTests'}
    <p class="main">{t('history.answer.fewTests')}</p>
    <p class="muted">{t('history.answer.fewTestsHint', { min: num(ANSWER_MIN_TESTS), n: num(answer.tests) })}</p>
  {:else if answer.kind === 'oneSet'}
    <p class="main">{t('history.answer.oneSet', { bands: label(answer.row) })}</p>
    <p>{t('history.answer.typical', { speed: speed(answer.row.medianDownMbps) })}</p>
    <p class="muted">{t('history.answer.oneSetHint')}</p>
  {:else if answer.kind === 'fastest'}
    <p class="main">{t('history.answer.fastest', { bands: label(answer.row) })}</p>
    <p>
      {t('history.answer.lead', { speed: speed(answer.row.medianDownMbps), pct: pct(answer.leadPct), other: label(answer.next) })}
      {#if answer.dayPart}{t(`history.answer.dayPart.${answer.dayPart}`)}{/if}
    </p>
  {:else}
    <p class="main">{t('history.answer.tooClose', { a: label(answer.a), b: label(answer.b) })}</p>
    <p class="muted">{t('history.answer.tooCloseHint', { pct: pct(Math.round(CLEAR_LEAD * 100)) })}</p>
  {/if}
  {#if basis}<p class="muted small">{basis}</p>{/if}

  <div class="actions">
    {#if tryRow}
      <button
        type="button"
        class="primary"
        disabled={!canTry}
        aria-label={t('history.bands.tryLabel', { bands: label(tryRow) })}
        onclick={() => tryRow && tryBands(tryRow.bands)}>{t('history.bands.try')}</button
      >
    {/if}
    {#if answer.kind !== 'fastest'}
      <button type="button" class={tryRow ? 'secondary' : 'primary'} onclick={() => (app.tab = 'scan')}>{t('history.answer.scan')}</button>
    {/if}
    {#if answer.kind === 'fastest' || answer.kind === 'tooClose'}
      <button type="button" class="link" onclick={seeTable}>{t('history.answer.seeTable')}</button>
    {/if}
  </div>
  {#if tryRow}<p class="muted small">{t('history.bands.tryHint')}</p>{/if}
</section>

<style>
  .answer {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding-block: var(--card-pad-block);
    padding-inline: var(--card-pad-inline);
    border: 1px solid var(--border);
    border-radius: var(--radius-card);
    background: var(--surface);
    box-shadow: var(--shadow-card);
    transition: opacity var(--t-fast) var(--ease);
  }
  .answer.stale {
    opacity: 0.6;
  }
  h2 {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    margin: 0;
    color: var(--muted);
    font-size: var(--fs-small);
    font-weight: 500;
  }
  h2 :global(svg) {
    color: var(--accent);
  }
  h2:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  p {
    margin: 0;
    max-inline-size: var(--measure);
  }
  .main {
    font-size: 22px;
    line-height: 30px;
    font-weight: 600;
  }
  .muted {
    color: var(--muted);
  }
  .small {
    font-size: var(--fs-small);
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px 12px;
    margin-block-start: 6px;
  }
  button.primary,
  button.secondary {
    min-block-size: var(--control-height);
    padding-inline: 14px;
    font-weight: 500;
  }
  button.secondary {
    background: var(--secondary);
    border-color: var(--secondary);
  }
  button.link {
    all: unset;
    color: var(--accent);
    text-decoration: underline;
    text-underline-offset: 2px;
    cursor: pointer;
    font-size: var(--fs-small);
  }
  button.link:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
</style>
