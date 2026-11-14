<script lang="ts">
  // Speed-test results as a table, newest first, a page at a time. The Speed test page and the
  // History page share it; History adds the column that says what started each test.
  import { tick, untrack } from 'svelte'
  import InfoTip from './ui/InfoTip.svelte'
  import { app } from '../lib/store.svelte'
  import { t } from '../lib/i18n'
  import { formatNumber } from '../lib/format'
  import type { SpeedTestResult } from '../lib/bindings/SpeedTestResult'
  import type { SpeedTestSource } from '../lib/bindings/SpeedTestSource'
  import { busyPing, incomplete, invalidReason, ipRangeLabel, latLevel, mbps, ms, notComparableKey, servingBands, servingDetail, whenLabel } from '../lib/speedtest.svelte'

  let {
    rows,
    sourceOf = null,
    open = null,
    caption,
    pageSize = 10,
  }: {
    /** Newest first. */
    rows: SpeedTestResult[]
    /** Shows a "Started by" column when given. */
    sourceOf?: ((r: SpeedTestResult) => SpeedTestSource) | null
    /** A result to bring into view: its page is shown and its row focused. A new object each time. */
    open?: { atMs: number } | null
    caption?: string
    pageSize?: number
  } = $props()

  const digits = $derived(app.settings?.digits ?? 'western')

  // A new result joins the first page, so a table being read stays put.
  let page = $state(0)
  const pages = $derived(Math.max(1, Math.ceil(rows.length / pageSize)))
  const shownPage = $derived(Math.min(page, pages - 1))
  const pageRows = $derived(rows.slice(shownPage * pageSize, (shownPage + 1) * pageSize))

  let body: HTMLTableSectionElement | undefined = $state()
  let opened: number | null = $state(null)
  $effect(() => {
    const want = open
    if (!want) return
    const i = untrack(() => rows.findIndex((r) => r.atMs === want.atMs))
    if (i < 0) return
    page = Math.floor(i / pageSize)
    opened = want.atMs
    void tick().then(() => {
      const row = body?.querySelector<HTMLTableRowElement>(`tr[data-at="${want.atMs}"]`)
      row?.scrollIntoView?.({ block: 'nearest' })
      row?.focus()
    })
  })
</script>

<div class="scroll">
  <table>
    {#if caption}<caption class="sr-only">{caption}</caption>{/if}
    <thead>
      <tr>
        <th scope="col">{t('speedtest.history.when')}</th>
        {#if sourceOf}<th scope="col">{t('speedtest.source.label')}</th>{/if}
        <th scope="col">{t('speedtest.history.serving')}</th>
        <th scope="col">
          {t('speedtest.history.ipRange')}
          <InfoTip label={t('metrics.whatIs', { name: t('speedtest.history.ipRange') })} text={t('speedtest.history.ipRangeHelp')} />
        </th>
        <th scope="col" class="n">{t('speedtest.phase.download')} <span class="th-unit">{t('speedtest.unit.mbps')}</span></th>
        <th scope="col" class="n">{t('speedtest.phase.upload')} <span class="th-unit">{t('speedtest.unit.mbps')}</span></th>
        <th scope="col" class="n">
          {t('speedtest.phase.idlePing')} <span class="th-unit">{t('speedtest.unit.ms')}</span>
          <InfoTip label={t('metrics.whatIs', { name: t('speedtest.phase.idlePing') })} text={t('metrics.help.idlePing')} />
        </th>
        <th scope="col" class="n">
          {t('speedtest.phase.busy')} <span class="th-unit">{t('speedtest.unit.ms')}</span>
          <InfoTip label={t('speedtest.busyHelpLabel')} text={t('speedtest.busyHelp')} />
        </th>
      </tr>
    </thead>
    <tbody bind:this={body}>
      {#each pageRows as r (r.atMs)}
        {@const b = busyPing(r)}
        {@const lvl = latLevel(b.ms)}
        <tr data-at={r.atMs} tabindex={opened === r.atMs ? -1 : undefined} class:opened={opened === r.atMs}>
          <td>{whenLabel(r.atMs, Date.now(), digits)}</td>
          {#if sourceOf}<td data-testid="source">{t(`speedtest.source.${sourceOf(r)}`)}</td>{/if}
          <td><bdi dir="ltr">{servingBands(r.serving)}</bdi> <span class="k">{servingDetail(r.serving)}</span></td>
          <td data-testid="ip-range">{#if ipRangeLabel(r)}<bdi dir="ltr" class="num">{ipRangeLabel(r)}</bdi>{:else}<span class="k">{t('app.unknown')}</span>{/if}</td>
          {#if invalidReason(r) && !incomplete(r)}
            <!-- The band or cell changed mid-test: the numbers mix two connections. -->
            <td colspan="4" class="k" data-testid="not-comparable">{t(notComparableKey(r))}</td>
          {:else}
            <td class="n strong-cell"><bdi dir="ltr">{mbps(r.downloadMbps, digits)}</bdi></td>
            <td class="n">
              <!-- The test stopped during its upload: the other numbers are real. -->
              {#if incomplete(r)}<span class="k" data-testid="upload-missing">{t('speedtest.history.uploadMissing')}</span
                >{:else}<bdi dir="ltr">{mbps(r.uploadMbps, digits)}</bdi>{/if}
            </td>
            <td class="n"><bdi dir="ltr">{ms(r.idlePingMs, digits)}</bdi></td>
            <td class="n">
              <bdi dir="ltr" class={lvl ? `lat-${lvl}` : ''}>{ms(b.ms, digits)}</bdi>
              {#if (lvl && lvl !== 'low') || b.during}
                <span class="k sub"
                  >{#if lvl && lvl !== 'low'}{t(`speedtest.lat.${lvl}`)}{/if}{#if lvl && lvl !== 'low' && b.during}{' · '}{/if}{#if b.during}{t(
                      b.during === 'upload' ? 'speedtest.whileUploading' : 'speedtest.whileDownloading',
                    )}{/if}</span
                >
              {/if}
            </td>
          {/if}
        </tr>
      {/each}
    </tbody>
  </table>
</div>
{#if pages > 1}
  <nav class="pager" aria-label={t('speedtest.history.pages')}>
    <button type="button" class="secondary" disabled={shownPage === 0} onclick={() => (page = shownPage - 1)}>{t('speedtest.history.newer')}</button>
    <span class="k">{t('speedtest.history.page', { n: formatNumber(shownPage + 1, { digits }), total: formatNumber(pages, { digits }) })}</span>
    <button type="button" class="secondary" disabled={shownPage >= pages - 1} onclick={() => (page = shownPage + 1)}>{t('speedtest.history.older')}</button>
  </nav>
{/if}

<style>
  .k {
    color: var(--muted);
    font-size: var(--fs-small);
  }
  .scroll {
    overflow-x: auto;
  }
  table {
    border-collapse: collapse;
    inline-size: 100%;
  }
  th {
    font-weight: 400;
    font-size: var(--fs-small);
    color: var(--muted);
    text-align: start;
    padding-block: 0 8px;
    padding-inline: 0 12px;
  }
  td {
    padding-block: 10px;
    padding-inline: 0 12px;
    border-block-start: 1px solid var(--rule);
    vertical-align: top;
  }
  th.n,
  td.n {
    text-align: end;
    font-variant-numeric: tabular-nums;
  }
  tr.opened td {
    background: var(--raised);
  }
  tr:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }
  .th-unit {
    color: var(--faint);
  }
  .strong-cell {
    font-weight: 500;
  }
  .sub {
    display: block;
  }
  .lat-high {
    color: var(--q-poor);
  }
  .lat-moderate {
    color: var(--q-fair);
  }
  .pager {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 12px;
    margin-block-start: 8px;
  }
  button.secondary {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    min-block-size: var(--control-height);
    padding-inline: 14px;
    background: var(--secondary);
    border-color: var(--secondary);
    font-weight: 500;
  }
</style>
