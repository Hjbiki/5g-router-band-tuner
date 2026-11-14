import { describe, expect, it, afterEach } from 'vitest'
import { readFileSync, readdirSync } from 'node:fs'
import { resolve } from 'node:path'
import { t, setLanguage, resolveLanguage, flagsByCode, LANGUAGES } from './i18n'

const dir = resolve(__dirname, 'i18n')
const langs: Record<string, Record<string, unknown>> = Object.fromEntries(
  readdirSync(dir)
    .filter((f: string) => f.endsWith('.json'))
    .map((f: string) => [f.slice(0, -5), JSON.parse(readFileSync(resolve(dir, f), 'utf8'))]),
)
const en = langs.en
const ar = langs.ar

function flatten(o: Record<string, unknown>, prefix = ''): Record<string, string> {
  const out: Record<string, string> = {}
  for (const [k, v] of Object.entries(o)) {
    const key = prefix ? `${prefix}.${k}` : k
    if (v !== null && typeof v === 'object') Object.assign(out, flatten(v as Record<string, unknown>, key))
    else out[key] = String(v)
  }
  return out
}

describe('i18n', () => {
  afterEach(() => setLanguage('en'))

  it('every language file has the same keys as en.json', () => {
    const want = Object.keys(flatten(en)).sort()
    for (const [code, json] of Object.entries(langs)) expect(Object.keys(flatten(json)).sort(), code).toEqual(want)
  })

  it('no empty values, and every file names itself', () => {
    for (const [name, json] of Object.entries(langs)) {
      for (const [k, v] of Object.entries(flatten(json))) expect(v.trim(), `${name}:${k}`).not.toBe('')
      const meta = json.meta as { dir: string }
      expect(['ltr', 'rtl'], name).toContain(meta.dir)
    }
  })

  it('finds the language files by themselves', () => {
    expect(LANGUAGES.map((l) => l.code)).toEqual(['en', 'ar'])
    expect(LANGUAGES.find((l) => l.code === 'ar')?.dir).toBe('rtl')
  })

  it('gives each language the flag file named by its code, and none when there is no file', () => {
    const flags = flagsByCode({ '../../assets/flags/en.svg': '/f/en.svg', '../../assets/flags/pt-BR.svg': '/f/pt-BR.svg' })
    expect(flags).toEqual({ en: '/f/en.svg', 'pt-BR': '/f/pt-BR.svg' })
    expect(flags.fr).toBeUndefined()
    // The two bundled languages have theirs.
    for (const l of LANGUAGES) expect(l.flag, l.code).toBeTruthy()
  })

  it('resolves the system language and falls back to English', () => {
    expect(resolveLanguage('system', ['ar-SA', 'en-US'])).toBe('ar')
    expect(resolveLanguage('system', ['fr-FR'])).toBe('en')
    expect(resolveLanguage('ar', ['en-US'])).toBe('ar')
    expect(resolveLanguage('xx', ['en-GB'])).toBe('en')
    expect(resolveLanguage(undefined, [])).toBe('en')
  })

  it('setLanguage("ar") sets dir=rtl and lang=ar', () => {
    setLanguage('ar')
    expect(document.documentElement.dir).toBe('rtl')
    expect(document.documentElement.lang).toBe('ar')
    setLanguage('en')
    expect(document.documentElement.dir).toBe('ltr')
    expect(document.documentElement.lang).toBe('en')
  })

  it('t interpolates {seconds}', () => {
    expect(t('safeApply.secondsLeft', { seconds: 42 })).toContain('42')
    setLanguage('ar')
    expect(t('safeApply.secondsLeft', { seconds: 42 })).toContain('42')
  })

  it('missing key throws in dev', () => {
    expect(() => t('nope.nothing' as never)).toThrow()
  })

  it('ui json contains all NOTIFY_KEYS', () => {
    const src = readFileSync(resolve(__dirname, '../../../src-tauri/src/notify.rs'), 'utf8')
    const block = /NOTIFY_KEYS[^=]*=\s*&\[([\s\S]*?)\];/.exec(src)
    expect(block).not.toBeNull()
    const keys = [...block![1].matchAll(/"([^"]+)"/g)].map((m) => m[1])
    expect(keys.length).toBeGreaterThan(0)
    const e = flatten(en)
    const a = flatten(ar)
    for (const k of keys) {
      expect(e[k], `en ${k}`).toBeTruthy()
      expect(a[k], `ar ${k}`).toBeTruthy()
    }
  })
})

describe('bindings', () => {
  it('bindings have no bigint', () => {
    const dir = resolve(__dirname, 'bindings')
    const files = readdirSync(dir).filter((f: string) => f.endsWith('.ts'))
    expect(files.length).toBeGreaterThan(10)
    for (const f of files) expect(readFileSync(resolve(dir, f), 'utf8'), f).not.toMatch(/bigint/)
  })
})
