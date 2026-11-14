// Runs in the page: opens the scene's tab, fills the chart, and sets the title once it's ready.
import { app } from '/src/lib/store.svelte.ts'

const scene = new URLSearchParams(location.search).get('scene') ?? 'signal'
const tabs = { signal: 'dashboard', bands: 'bands', scan: 'scan', speed: 'speedtest' }
const now = Date.now()

setTimeout(async () => {
  app.tab = tabs[scene]
  app.history = Array.from({ length: 120 }, (_, i) => ({
    t: now - (120 - i) * 2500,
    lteSinr: 8 + Math.sin(i / 6) * 2.4 + Math.sin(i / 2.3) * 0.7,
    lteRsrp: -95 + Math.sin(i / 11) * 1.5,
    nrSinr: 16 + Math.sin(i / 8) * 2.6 + Math.sin(i / 3.1) * 0.8,
    nrRsrp: -88 + Math.sin(i / 13) * 1.5,
  }))
  await new Promise((r) => setTimeout(r, 500))
  if (scene === 'scan') document.querySelector('[data-testid="scan-results"]')?.scrollIntoView({ block: 'start' })
  document.title = 'ready'
}, 200)
