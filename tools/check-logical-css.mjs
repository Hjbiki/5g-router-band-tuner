// Fails if any .svelte/.css under ui/src uses physical (non-logical) CSS properties.
import { readdirSync, readFileSync, statSync } from 'node:fs'
import { dirname, join, relative, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const srcDir = join(root, 'ui', 'src')
const FORBIDDEN =
  /margin-left|margin-right|padding-left|padding-right|(?<![\w-])left\s*:|(?<![\w-])right\s*:|text-align\s*:\s*(left|right)/

function* walk(dir) {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name)
    if (statSync(p).isDirectory()) yield* walk(p)
    else if (p.endsWith('.svelte') || p.endsWith('.css')) yield p
  }
}

let bad = 0
for (const file of walk(srcDir)) {
  readFileSync(file, 'utf8')
    .split(/\r?\n/)
    .forEach((line, i) => {
      if (FORBIDDEN.test(line)) {
        console.error(`${relative(root, file)}:${i + 1}: physical CSS property: ${line.trim()}`)
        bad++
      }
    })
}
if (bad) {
  console.error(`${bad} physical CSS use(s) found; use logical properties (inline-start/end).`)
  process.exit(1)
}
console.log('logical CSS check passed')
