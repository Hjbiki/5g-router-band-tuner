<script lang="ts">
  // The band picker in the Spectrum design: every 4G band the router supports in one row, from low
  // frequency to high, with the download speed each gave when it was last measured here. It only
  // edits the draft through `onToggle`; applying stays with the Bands panel.
  import type { NetConfig } from '../../lib/bindings/NetConfig'
  import type { BandRecord } from '../../lib/bandHistory'
  import { app } from '../../lib/store.svelte'
  import { t } from '../../lib/i18n'
  import { LTE_BAND_INFO, bandChange, hasBand } from '../../lib/bands'
  import { formatNumber, isolate } from '../../lib/format'
  import { mbps } from '../../lib/speedtest.svelte'

  let {
    supported,
    draft,
    current,
    serving,
    records,
    disabled = false,
    onToggle,
  }: {
    supported: number[]
    draft: NetConfig
    current: NetConfig | null
    serving: number | null
    /** The last recorded result per band; a band with none has not been tried here. */
    records: Map<number, BandRecord | null>
    disabled?: boolean
    onToggle: (band: number) => void
  } = $props()

  const digits = $derived(app.settings?.digits ?? 'western')
  const num = (n: number) => formatNumber(n, { digits })
  const BAR_MAX_PX = 130

  const mhzOf = (b: number) => LTE_BAND_INFO[b]?.mhz ?? 0
  const bands = $derived([...supported].sort((a, b) => mhzOf(a) - mhzOf(b) || a - b))
  const fastest = $derived(Math.max(1, ...bands.map((b) => records.get(b)?.downloadMbps ?? 0)))
  const cols = $derived(
    bands.map((b) => {
      const r = records.get(b) ?? null
      const speed = r?.kind === 'measured' ? r.downloadMbps : null
      const delta = bandChange(draft, current, b)
      return {
        b,
        mhz: mhzOf(b),
        on: hasBand(draft, b),
        speed,
        barPx: speed === null ? 0 : Math.max(3, Math.round((speed / fastest) * BAR_MAX_PX)),
        fiveG: r?.got5g ?? false,
        none: r?.kind === 'noService',
        downOnly: LTE_BAND_INFO[b]?.duplex === 'sdl',
        tag: delta === 'added' ? t('spectrum.map.adding') : delta === 'removed' ? t('spectrum.map.removing') : serving === b ? t('spectrum.map.now') : '',
      }
    }),
  )
  // Low, mid and high: what the frequency means for reach and speed.
  const groups = $derived(
    (
      [
        ['spectrum.map.low', cols.filter((c) => c.mhz < 1000).length],
        ['spectrum.map.mid', cols.filter((c) => c.mhz >= 1000 && c.mhz < 3000).length],
        ['spectrum.map.high', cols.filter((c) => c.mhz >= 3000).length],
      ] as const
    ).filter(([, n]) => n > 0),
  )
  const tried = $derived(cols.filter((c) => records.get(c.b)).length)
  const title = (c: (typeof cols)[number]) =>
    [
      c.speed !== null ? t('speedtest.unit.speed', { n: mbps(c.speed, digits) }) : c.none ? t('bands.legend.noService') : t('spectrum.map.untriedShort'),
      c.downOnly ? t('bands.downOnly') : null,
    ]
      .filter(Boolean)
      .join(' · ')
</script>

<div class="map" data-testid="band-map">
  <div class="top">
    <h2>{t('spectrum.map.title')}</h2>
    <span class="k">{t('spectrum.map.bars')}</span>
  </div>

  <!-- A frequency axis reads low to high from the left in every language. -->
  <div class="scroll" dir="ltr">
    <div class="strip" style:--n={cols.length}>
      <div class="bars" aria-hidden="true">
        {#each cols as c (c.b)}
          <div class="col">
            {#if c.speed !== null}
              <span class="speed">{mbps(c.speed, digits)}</span>
              <div class="bar" class:five={c.fiveG} style:block-size="{c.barPx}px"></div>
            {:else if c.none}
              <span class="none" dir="auto">{t('spectrum.map.noService')}</span>
            {/if}
          </div>
        {/each}
      </div>
      <div class="cells">
        {#each cols as c (c.b)}
          <button type="button" class="band" class:on={c.on} aria-pressed={c.on} {disabled} title={title(c)} onclick={() => onToggle(c.b)}>
            <span class="name">B{c.b}</span>
            <span class="mhz">{num(c.mhz)}</span>
          </button>
        {/each}
      </div>
      <div class="tags">
        {#each cols as c (c.b)}<span class="tag" dir="auto">{c.tag}</span>{/each}
      </div>
      <div class="groups">
        {#each groups as [key, n] (key)}<span class="group" dir="auto" style:grid-column="span {n}">{t(key)}</span>{/each}
      </div>
    </div>
  </div>

  <ul class="key">
    <li><i class="sw on"></i>{t('spectrum.map.allowed')}</li>
    <li><i class="sw"></i>{t('spectrum.map.notAllowed')}</li>
    <li><i class="sw bar five"></i>{t('spectrum.map.with5g')}</li>
    <li><i class="sw bar"></i>{t('spectrum.map.only4g')}</li>
    <li>{t('spectrum.map.untried')}</li>
  </ul>

  <div class="foot">
    <span>{t('spectrum.map.tried', { n: isolate(num(tried)), total: isolate(num(cols.length)) })}</span>
    <button type="button" class="pill" onclick={() => (app.tab = 'scan')}>{t('spectrum.map.scan')}</button>
  </div>
</div>

<style>
  .map {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }
  .top {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    justify-content: space-between;
    gap: 4px 16px;
  }
  h2 {
    margin: 0;
    font-size: var(--fs-body);
    font-weight: 600;
    line-height: 22px;
  }
  .k,
  .key,
  .group {
    color: var(--muted);
    font-size: var(--fs-small);
  }
  .scroll {
    overflow-x: auto;
  }
  .strip {
    min-inline-size: 640px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .bars,
  .cells,
  .tags,
  .groups {
    display: grid;
    grid-template-columns: repeat(var(--n), minmax(0, 1fr));
    column-gap: 6px;
  }
  .bars {
    block-size: 168px;
    align-items: end;
  }
  .col {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: flex-end;
    gap: 4px;
    block-size: 100%;
  }
  .speed {
    font-size: var(--fs-small);
    font-weight: 600;
    font-variant-numeric: tabular-nums;
  }
  .bar {
    inline-size: 100%;
    background: var(--faint);
    border-start-start-radius: 4px;
    border-start-end-radius: 4px;
  }
  .bar.five {
    background: var(--accent);
  }
  .none {
    font-size: 12px;
    line-height: 16px;
    color: var(--q-poor);
    text-align: center;
  }
  .band {
    all: unset;
    box-sizing: border-box;
    min-block-size: 56px;
    padding-block: 8px;
    padding-inline: 2px;
    border-radius: 8px;
    background: var(--secondary);
    color: var(--muted);
    display: flex;
    flex-direction: column;
    align-items: center;
    cursor: pointer;
    transition:
      background-color var(--t-fast) var(--ease),
      color var(--t-fast) var(--ease);
  }
  /* Allowed is the inverted cell: the strongest mark on the map. */
  .band.on {
    background: var(--primary);
    color: var(--primary-ink);
  }
  .band:hover:not(:disabled):not(.on) {
    color: var(--text);
  }
  .band:disabled {
    cursor: default;
    opacity: 0.6;
  }
  .band:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  .name {
    font-weight: 600;
  }
  .mhz {
    font-size: 12px;
    line-height: 16px;
    opacity: 0.8;
    font-variant-numeric: tabular-nums;
  }
  .tags {
    min-block-size: 18px;
  }
  .tag {
    text-align: center;
    font-size: 12px;
    line-height: 16px;
    color: var(--accent);
    font-weight: 500;
  }
  .group {
    border-block-start: 2px solid var(--control);
    padding-block-start: 6px;
  }
  .key {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px 20px;
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .key li {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .sw {
    display: block;
    inline-size: 14px;
    block-size: 14px;
    border-radius: 4px;
    background: var(--secondary);
  }
  .sw.on {
    background: var(--primary);
  }
  .sw.bar {
    block-size: 10px;
    border-end-start-radius: 0;
    border-end-end-radius: 0;
    background: var(--faint);
  }
  .sw.bar.five {
    background: var(--accent);
  }
  .foot {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding-block-start: 16px;
    border-block-start: 1px solid var(--rule);
  }
  .pill {
    block-size: var(--control-height);
    padding-inline: 16px;
    border-radius: 999px;
    font-weight: 500;
  }
</style>
