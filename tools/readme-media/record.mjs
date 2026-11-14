// Records the README's screenshots and GIFs from the real UI with the made-up backend in
// demo-backend.js. See README.md here. Usage: node tools/readme-media/record.mjs [en] [ar]
//
// Talks to localhost only: no Tauri, no router. The browser is Playwright's own Chromium, never an
// installed browser, on a profile made for this run and deleted at its end, so nothing synced to
// an account can reach it. A capture fails if the page shows a PCI the demo driver doesn't have,
// or any operator name.
import { spawnSync } from 'node:child_process'
import { existsSync, mkdirSync, mkdtempSync, rmSync, statSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

import { writeDiagrams } from './diagrams.mjs'

const HERE = dirname(fileURLToPath(import.meta.url))
const ROOT = resolve(HERE, '../..')
const UI = join(ROOT, 'ui')
const MEDIA = join(ROOT, 'docs/media')
const WORK = join(ROOT, 'target/readme-media')
const VITE_PORT = 5198
const BROWSER_ARGS = ['--disable-extensions', '--disable-sync']
const MAX_GIF_BYTES = 5 * 1024 * 1024

const DEMO_PCIS = new Set(['101', '103', '108', '120', '128', '140'])
const OPERATORS = /Zain|\bstc\b|Mobily|Salam|Lebara|Virgin|موبايلي|سلام/i

const fwd = (p) => p.replaceAll('\\', '/')
const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

function checkDemoOnly(text, where) {
  for (const m of text.matchAll(/PCI\s*(\d+)/g)) {
    if (!DEMO_PCIS.has(m[1])) throw new Error(`PCI ${m[1]} isn't a demo value: ${where}`)
  }
  const op = OPERATORS.exec(text)
  if (op) throw new Error(`operator name "${op[0]}" on screen: ${where}`)
}

async function startVite() {
  const load = (p) => import(pathToFileURL(join(UI, 'node_modules', p)).href)
  const { createServer } = await load('vite/dist/node/index.js')
  const { svelte } = await load('@sveltejs/vite-plugin-svelte/src/index.js')
  const backend = fwd(join(HERE, 'demo-backend.js'))
  const server = await createServer({
    configFile: false,
    root: fwd(UI),
    cacheDir: fwd(join(WORK, 'vite')),
    logLevel: 'error',
    plugins: [
      svelte({ configFile: false }),
      {
        name: 'readme-media-setup',
        transformIndexHtml: () => [
          { tag: 'script', attrs: { type: 'module', src: `/@fs/${fwd(join(HERE, 'page-setup.js'))}` }, injectTo: 'body' },
        ],
      },
    ],
    resolve: {
      alias: Object.fromEntries(['core', 'event', 'app', 'webview', 'window'].map((m) => [`@tauri-apps/api/${m}`, backend])),
    },
    server: { port: VITE_PORT, strictPort: true, fs: { strict: false } },
  })
  await server.listen()
  return server
}

class Tab {
  constructor(context, page) {
    this.context = context
    this.page = page
  }

  /** One browser per capture, since a profile's window size and pixel scale are set at launch. */
  static async open(chromium, profile, { w, h, dsf }) {
    const context = await chromium.launchPersistentContext(profile, {
      headless: true,
      args: BROWSER_ARGS,
      viewport: { width: w, height: h },
      deviceScaleFactor: dsf,
    })
    return new Tab(context, context.pages()[0] ?? (await context.newPage()))
  }

  eval(expression) {
    return this.page.evaluate(expression)
  }

  async load(params) {
    await this.page.goto(`http://localhost:${VITE_PORT}/?${new URLSearchParams(params)}`)
    await this.page.waitForFunction(() => document.title === 'ready', undefined, { timeout: 30_000 })
    await this.eval('document.fonts.ready.then(() => 1)')
    await sleep(500)
    await this.check(JSON.stringify(params))
  }

  async check(where) {
    checkDemoOnly(await this.eval('document.body.innerText'), where)
  }

  png(path) {
    return this.page.screenshot({ path, type: 'png', animations: 'allow' })
  }

  close() {
    return this.context.close()
  }
}

async function screenshot(session, lang, name, scene) {
  const t = await Tab.open(session.chromium, session.profile, { w: 1280, h: 800, dsf: 1.5 })
  await t.load({ scene, lang })
  const out = join(MEDIA, lang, name)
  await t.png(out)
  await t.close()
  console.log(`${lang}/${name}  ${Math.round(statSync(out).size / 1024)} KB`)
}

async function gif(session, lang, name, scene, seconds) {
  const frames = join(WORK, 'frames', lang, name)
  rmSync(frames, { recursive: true, force: true })
  mkdirSync(frames, { recursive: true })
  const t = await Tab.open(session.chromium, session.profile, { w: 1200, h: 750, dsf: 1 })
  await t.load({ scene, lang })
  await t.eval('window.__go()')
  const start = Date.now()
  const stamps = []
  while (Date.now() - start < seconds * 1000) {
    const at = Date.now()
    await t.png(join(frames, `${String(stamps.length).padStart(4, '0')}.png`))
    if (stamps.length % 6 === 0) await t.check(`${lang}/${name}`)
    stamps.push(at)
    const gap = 100 - (Date.now() - at)
    if (gap > 0) await sleep(gap)
  }
  await t.check(`${lang}/${name}`)
  await t.close()

  const frame = (i) => `${String(i).padStart(4, '0')}.png`
  let list = ''
  stamps.forEach((at, i) => {
    const next = i + 1 < stamps.length ? stamps[i + 1] : at + 100
    list += `file '${frame(i)}'\nduration ${((next - at) / 1000).toFixed(3)}\n`
  })
  list += `file '${frame(stamps.length - 1)}'\n`
  writeFileSync(join(frames, 'list.txt'), list)

  const out = join(MEDIA, lang, name)
  const vf = 'fps=10,scale=1000:-1:flags=lanczos,split[a][b];[a]palettegen=max_colors=96:stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=5:diff_mode=rectangle'
  const r = spawnSync('ffmpeg', ['-y', '-loglevel', 'error', '-f', 'concat', '-safe', '0', '-i', join(frames, 'list.txt'), '-vf', vf, '-loop', '0', out], { encoding: 'utf8' })
  if (r.error) throw new Error(`ffmpeg could not be started: ${r.error.message}`)
  if (r.status !== 0) throw new Error(`ffmpeg: ${r.stderr}`)
  const size = statSync(out).size
  if (size > MAX_GIF_BYTES) throw new Error(`${lang}/${name} is ${(size / 1048576).toFixed(1)} MB, over the 5 MB limit`)
  console.log(`${lang}/${name}  ${(size / 1048576).toFixed(2)} MB`)
}

async function recordLanguage(session, lang) {
  mkdirSync(join(MEDIA, lang), { recursive: true })
  await screenshot(session, lang, 'signal.png', 'signal')
  await screenshot(session, lang, 'auto-scan-results.png', 'scan')
  await gif(session, lang, 'signal-live.gif', 'signal', 7)
  await gif(session, lang, 'safe-apply.gif', 'bands', 9.5)
  await gif(session, lang, 'speed-test.gif', 'speed', 9)
}

const langs = process.argv.slice(2).length ? process.argv.slice(2) : ['en', 'ar']
for (const lang of langs) {
  if (!['en', 'ar'].includes(lang)) {
    console.error(`unknown language ${lang}: this rig records en and ar`)
    process.exit(1)
  }
}
const { chromium } = await import(pathToFileURL(join(UI, 'node_modules/playwright/index.mjs')).href)
if (!existsSync(chromium.executablePath())) {
  console.error('Playwright\'s Chromium is missing. Run: pnpm --dir ui exec playwright install chromium')
  process.exit(1)
}

const profile = mkdtempSync(join(tmpdir(), 'readme-media-profile-'))
const removeProfile = () => rmSync(profile, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 })
process.on('SIGINT', () => {
  removeProfile()
  process.exit(130)
})

let code = 0
let vite
try {
  mkdirSync(WORK, { recursive: true })
  for (const lang of langs) mkdirSync(join(MEDIA, lang), { recursive: true })
  writeDiagrams(MEDIA, langs)
  vite = await startVite()
  for (const lang of langs) await recordLanguage({ chromium, profile }, lang)
} catch (e) {
  console.error(`FAILED: ${e.message}`)
  code = 1
} finally {
  await vite?.close()
  removeProfile()
  if (existsSync(profile)) {
    console.error(`FAILED: couldn't delete the browser profile at ${profile}`)
    code = 1
  }
}
process.exit(code)
