<script lang="ts">
  // A band's past result as an icon with its words for screen readers, so it never rests on colour.
  import Ban from '@lucide/svelte/icons/ban'
  import CircleCheck from '@lucide/svelte/icons/circle-check'
  import Snail from '@lucide/svelte/icons/snail'
  import { t } from '../../lib/i18n'
  import { TONE_LABEL_KEY, type BandTone } from '../../lib/bandHistory'

  let { tone, size = 13 }: { tone: BandTone; size?: number } = $props()
  const ICON = { good: CircleCheck, slow: Snail, noService: Ban } as const
  const Icon = $derived(ICON[tone])
</script>

<span class="tone-mark tone-{tone}"><Icon {size} aria-hidden="true" /><span class="sr-only">{t(TONE_LABEL_KEY[tone])}</span></span>

<style>
  .tone-mark {
    display: inline-flex;
    color: var(--tone-ink);
  }
  .tone-good {
    --tone-ink: var(--q-excellent);
  }
  .tone-slow {
    --tone-ink: var(--q-fair);
  }
  .tone-noService {
    --tone-ink: var(--q-poor);
  }
</style>
