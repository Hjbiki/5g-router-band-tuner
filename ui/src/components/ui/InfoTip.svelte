<script lang="ts">
  import type { Snippet } from 'svelte'
  import Info from '@lucide/svelte/icons/info'

  // A small "what is this?" button with a one-sentence popover anchored to it. The popover floats
  // over the page, so opening it never moves anything. It opens when the pointer rests on the
  // button, and lasts while the pointer is on the button or the popover; a click keeps it open
  // until a second click. A click elsewhere, Esc, scrolling or a window resize closes it either
  // way. The text is plain; no HTML. `children` replaces it for a small layout of spans.
  let { label, text = '', children }: { label: string; text?: string; children?: Snippet } = $props()
  const id = $props.id()
  const WIDTH_GUTTER = 8
  // Long enough that crossing a row of these opens none of them.
  const OPEN_DELAY_MS = 150
  // Long enough to cross the gap between the button and the popover.
  const CLOSE_DELAY_MS = 200

  let pinned = $state(false)
  let hovered = $state(false)
  const open = $derived(pinned || hovered)
  let hoverTimer: ReturnType<typeof setTimeout> | undefined
  let button: HTMLButtonElement | undefined = $state()
  let tip: HTMLSpanElement | undefined = $state()
  let side: 'above' | 'below' = $state('below')
  let block = $state(0)
  let inline = $state(0)
  let arrow = $state(0)

  /** Places the popover by the button, kept on screen. Inline values count from the reading start. */
  function place() {
    if (!button || !tip) return
    const b = button.getBoundingClientRect()
    const t = tip.getBoundingClientRect()
    const vw = document.documentElement.clientWidth
    const rtl = getComputedStyle(document.documentElement).direction === 'rtl'
    const centre = (rtl ? vw - (b.left + b.right) / 2 : (b.left + b.right) / 2)
    const start = Math.max(WIDTH_GUTTER, Math.min(centre - t.width / 2, vw - t.width - WIDTH_GUTTER))
    inline = start
    arrow = Math.max(8, Math.min(centre - start - 5, t.width - 18))
    const below = b.bottom + 8 + t.height <= window.innerHeight - WIDTH_GUTTER
    side = below ? 'below' : 'above'
    block = below ? b.bottom + 8 : b.top - 8 - t.height
  }

  function close() {
    clearTimeout(hoverTimer)
    pinned = false
    hovered = false
  }

  /** The pointer came onto the button or the popover (`true`), or left it. */
  function hover(on: boolean) {
    clearTimeout(hoverTimer)
    hoverTimer = setTimeout(() => (hovered = on), on ? OPEN_DELAY_MS : CLOSE_DELAY_MS)
  }

  $effect(() => () => clearTimeout(hoverTimer))

  $effect(() => {
    if (!open) return
    place()
    const onClick = (e: MouseEvent) => {
      const target = e.target as Node
      if (!button?.contains(target) && !tip?.contains(target)) close()
    }
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return
      close()
      button?.focus()
    }
    // Tabbing on closes it, so it never sits over the next control.
    const onFocus = (e: FocusEvent) => {
      const target = e.target as Node
      if (!button?.contains(target) && !tip?.contains(target)) close()
    }
    document.addEventListener('click', onClick, true)
    document.addEventListener('focusin', onFocus)
    window.addEventListener('keydown', onKey)
    window.addEventListener('scroll', close, true)
    window.addEventListener('resize', close)
    return () => {
      document.removeEventListener('click', onClick, true)
      document.removeEventListener('focusin', onFocus)
      window.removeEventListener('keydown', onKey)
      window.removeEventListener('scroll', close, true)
      window.removeEventListener('resize', close)
    }
  })
</script>

<span class="wrap">
  <button
    type="button"
    class="info"
    bind:this={button}
    aria-label={label}
    aria-expanded={open}
    aria-controls={id}
    onmouseenter={() => hover(true)}
    onmouseleave={() => hover(false)}
    onclick={(e) => {
      // Inside a <label> the click must not reach its control as well.
      e.preventDefault()
      // A click keeps it open, whether or not the pointer had opened it; the next one closes it.
      if (pinned) close()
      else {
        clearTimeout(hoverTimer)
        pinned = true
      }
    }}
  >
    <Info size={15} aria-hidden="true" />
  </button>
  {#if open}
    <span
      {id}
      bind:this={tip}
      class="term-tip {side}"
      role="note"
      onmouseenter={() => hover(true)}
      onmouseleave={() => hover(false)}
      style:inset-block-start="{block}px"
      style:inset-inline-start="{inline}px"
      style:--arrow="{arrow}px">{#if children}{@render children()}{:else}{text}{/if}</span
    >
  {/if}
</span>

<style>
  .wrap {
    display: contents;
  }
  .info {
    all: unset;
    display: inline-grid;
    place-items: center;
    inline-size: 24px;
    block-size: 24px;
    flex: none;
    border-radius: 5px;
    color: var(--muted);
    cursor: pointer;
    vertical-align: middle;
    transition:
      background-color var(--t-fast) var(--ease),
      color var(--t-fast) var(--ease);
  }
  .info:hover:not(:disabled),
  .info[aria-expanded='true'] {
    background: var(--active);
    color: var(--text);
  }
  .info:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
</style>
