<svelte:options namespace="svg" />

<script lang="ts">
  // The mark on top of a chart's upright line, so its kind never rests on colour alone: a tick
  // for a kept band change, a cross for one undone, a ring for one tried, a square for an app
  // start and a diamond for a router restart.
  let { kind, cx, cy }: { kind: string; cx: number; cy: number } = $props()
  const f = (n: number) => n.toFixed(1)
</script>

<g class="glyph {kind}" data-glyph={kind}>
  {#if kind === 'kept'}
    <path d="M{f(cx - 4)} {f(cy)} L{f(cx - 1)} {f(cy + 3)} L{f(cx + 4)} {f(cy - 3)}" />
  {:else if kind === 'reverted'}
    <path d="M{f(cx - 3.5)} {f(cy - 3.5)} L{f(cx + 3.5)} {f(cy + 3.5)} M{f(cx + 3.5)} {f(cy - 3.5)} L{f(cx - 3.5)} {f(cy + 3.5)}" />
  {:else if kind === 'appStart'}
    <rect x={cx - 3.5} y={cy - 3.5} width="7" height="7" />
  {:else if kind === 'reboot'}
    <polygon points="{f(cx)},{f(cy - 4.5)} {f(cx + 4.5)},{f(cy)} {f(cx)},{f(cy + 4.5)} {f(cx - 4.5)},{f(cy)}" />
  {:else}
    <circle {cx} {cy} r="3.5" />
  {/if}
</g>

<style>
  .glyph {
    fill: var(--surface);
    stroke: var(--muted);
    stroke-width: 1.75;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  .glyph path {
    fill: none;
  }
  .glyph.kept {
    stroke: var(--q-excellent);
  }
  .glyph.reverted {
    stroke: var(--q-poor);
  }
</style>
