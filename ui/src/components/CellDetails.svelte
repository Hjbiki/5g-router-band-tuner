<script lang="ts">
  // Both links side by side, one line each.
  import TableProperties from '@lucide/svelte/icons/table-properties'
  import ChevronDown from '@lucide/svelte/icons/chevron-down'
  import type { LteSignal } from '../lib/bindings/LteSignal'
  import type { NrSignal } from '../lib/bindings/NrSignal'
  import { app } from '../lib/store.svelte'
  import { t, type MessageKey } from '../lib/i18n'
  import { formatMeasure, formatNumber } from '../lib/format'
  import InfoTip from './ui/InfoTip.svelte'

  let { lte, nr }: { lte: LteSignal | null; nr: NrSignal | null } = $props()

  // A per-viewer convenience only: the page works the same without storage.
  const KEY = 'cell-details-open'
  const stored = () => {
    try {
      return localStorage.getItem(KEY) === '1'
    } catch {
      return false
    }
  }
  let open = $state(stored())
  $effect(() => {
    try {
      localStorage.setItem(KEY, open ? '1' : '0')
    } catch {
      // Storage is blocked; the section just starts closed next time.
    }
  })

  const digits = $derived(app.settings?.digits ?? 'western')
  const num = (n: number | null) => formatNumber(n, { digits })
  const measure = (n: number | null, unit: 'dBm' | 'MHz') => formatMeasure(n, unit, digits)

  const COLS: { id: string; name: () => string; help: MessageKey }[] = [
    { id: 'pci', name: () => 'PCI', help: 'metrics.help.pci' },
    { id: 'arfcn', name: () => t('dashboard.arfcn'), help: 'metrics.help.arfcn' },
    { id: 'enb', name: () => t('dashboard.enbSector'), help: 'metrics.help.enbSector' },
    { id: 'width', name: () => t('dashboard.bandwidth'), help: 'metrics.help.bandwidth' },
    { id: 'tx', name: () => t('metrics.uploadPower'), help: 'metrics.help.txPower' },
    { id: 'mcs', name: () => t('dashboard.mcs'), help: 'metrics.help.mcs' },
    { id: 'cqi', name: () => t('dashboard.cqi'), help: 'metrics.help.cqi' },
  ]

  type Common = LteSignal | NrSignal
  const row = (name: string, s: Common, arfcn: number | null, enb: string) => ({
    name,
    cells: [
      num(s.pci),
      num(arfcn),
      enb,
      measure(s.dlBwMhz, 'MHz'),
      measure(s.txPower, 'dBm'),
      `${num(s.dlMcs[0])} / ${num(s.dlMcs[1])} · ${num(s.ulMcs)}`,
      `${num(s.cqi[0])} / ${num(s.cqi[1])}`,
    ],
  })
  const rows = $derived([
    ...(lte
      ? [
          row(
            `4G · ${lte.band != null ? `B${lte.band}` : '—'}`,
            lte,
            lte.earfcnDl,
            lte.enbId != null && lte.sector != null ? `${num(lte.enbId)}/${num(lte.sector)}` : '—',
          ),
        ]
      : []),
    ...(nr ? [row(`5G · ${nr.band ? `n${nr.band.primary}` : '—'}`, nr, nr.arfcnDl, '—')] : []),
  ])
</script>

{#if rows.length > 0}
  <!-- For advanced users: folded away until asked for, and it stays as the user left it. -->
  <details class="card" bind:open data-testid="cell-details">
    <summary>
      <TableProperties size={18} aria-hidden="true" />
      <span class="title">{t('dashboard.cellDetails')}</span>
      <span class="adv">{t('dashboard.cellDetailsAdvanced')}</span>
      <ChevronDown class="chevron" size={16} aria-hidden="true" />
    </summary>
    <div class="scroll">
      <table>
        <thead>
          <tr>
            <th scope="col">{t('dashboard.link')}</th>
            {#each COLS as c (c.id)}
              <th scope="col" class="n">
                <span class="head">
                  {c.name()}
                  <InfoTip label={t('metrics.whatIs', { name: c.name() })} text={t(c.help)} />
                </span>
              </th>
            {/each}
          </tr>
        </thead>
        <tbody>
          {#each rows as r (r.name)}
            <tr>
              <td><bdi dir="ltr">{r.name}</bdi></td>
              {#each r.cells as cell, i (i)}<td class="n"><bdi dir="ltr">{cell}</bdi></td>{/each}
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  </details>
{/if}

<style>
  summary {
    display: flex;
    align-items: center;
    gap: 8px;
    cursor: pointer;
    list-style: none;
    font-size: var(--fs-section);
    font-weight: 600;
  }
  summary::-webkit-details-marker {
    display: none;
  }
  summary > :global(svg:first-child) {
    color: var(--accent);
    flex: none;
  }
  summary:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 4px;
    border-radius: 4px;
  }
  .adv {
    color: var(--muted);
    font-size: var(--fs-small);
    font-weight: 400;
  }
  summary :global(.chevron) {
    margin-inline-start: auto;
    color: var(--muted);
    transition: transform var(--t-fast) var(--ease);
  }
  details[open] summary :global(.chevron) {
    transform: rotate(180deg);
  }
  .card {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius-card);
    padding-block: var(--card-pad-block);
    padding-inline: var(--card-pad-inline);
    display: flex;
    flex-direction: column;
    gap: 10px;
    min-inline-size: 0;
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
    white-space: nowrap;
    padding-block: 0 8px;
    padding-inline-start: 16px;
  }
  /* In a narrow window the columns scroll sideways, and the row's name stays beside them. */
  th:first-child,
  td:first-child {
    position: sticky;
    inset-inline-start: 0;
    z-index: 1;
    background: var(--surface);
    padding-inline: 0 12px;
  }
  td {
    padding-block: 10px;
    padding-inline-start: 16px;
    border-block-start: 1px solid var(--rule);
    white-space: nowrap;
  }
  .n {
    text-align: end;
    font-variant-numeric: tabular-nums;
  }
  .head {
    display: inline-flex;
    align-items: center;
    gap: 2px;
  }
</style>
