import en from './en.json'
import { langState, type Lang } from './lang.svelte'

type Paths<T> = {
  [K in keyof T & string]: T[K] extends string ? K : `${K}.${Paths<T[K]>}`
}[keyof T & string]

/** Flattened dotted keys, e.g. "dashboard.sinr". English is the reference file. */
export type MessageKey = Paths<typeof en>

/**
 * Every `<code>.json` next to this file is a language. To add one, copy `en.json` to
 * `<code>.json` (for example `fr.json`), translate the values and set `meta.name` (the
 * language's own name) and `meta.dir` (`ltr` or `rtl`). Tests check that its keys match.
 */
const files = import.meta.glob<Record<string, unknown>>('./*.json', { eager: true, import: 'default' })

const dictionaries: Record<Lang, Record<string, unknown>> = {}
for (const [path, dict] of Object.entries(files)) {
  dictionaries[path.slice(2, -'.json'.length)] = dict
}

export type LanguageInfo = { code: Lang; name: string; dir: 'ltr' | 'rtl' }

/** The available languages, English first, then by code. */
export const LANGUAGES: LanguageInfo[] = Object.entries(dictionaries)
  .map(([code, d]) => {
    const meta = (d.meta ?? {}) as { name?: string; dir?: string }
    return { code, name: meta.name ?? code, dir: meta.dir === 'rtl' ? 'rtl' : 'ltr' } as LanguageInfo
  })
  .sort((a, b) => (a.code === 'en' ? -1 : b.code === 'en' ? 1 : a.code.localeCompare(b.code)))

function lookup(dict: Record<string, unknown> | undefined, key: string): string | undefined {
  let cur: unknown = dict
  for (const part of key.split('.')) {
    if (cur === null || typeof cur !== 'object') return undefined
    cur = (cur as Record<string, unknown>)[part]
  }
  return typeof cur === 'string' ? cur : undefined
}

/**
 * The keys under `prefix` (without it), each with its text in `lang`, falling back to English.
 * English is the reference file, so it decides which keys exist.
 */
export function messagesUnder(prefix: string, lang: Lang = langState.lang): Record<string, string> {
  const out: Record<string, string> = {}
  const walk = (node: unknown, path: string) => {
    if (typeof node === 'string') {
      out[path] = lookup(dictionaries[lang], `${prefix}.${path}`) ?? node
    } else if (node !== null && typeof node === 'object') {
      for (const [k, v] of Object.entries(node)) walk(v, path ? `${path}.${k}` : k)
    }
  }
  let root: unknown = dictionaries.en
  for (const part of prefix.split('.')) root = (root as Record<string, unknown> | undefined)?.[part]
  if (root !== null && typeof root === 'object') walk(root, '')
  return out
}

export function t(key: MessageKey, vars?: Record<string, string | number>): string {
  // A language file that lacks a key falls back to English.
  const text = lookup(dictionaries[langState.lang], key) ?? lookup(dictionaries.en, key)
  if (text === undefined) {
    if (import.meta.env.DEV) throw new Error(`missing i18n key: ${key}`)
    return key
  }
  if (!vars) return text
  return text.replace(/\{(\w+)\}/g, (m, name: string) => (name in vars ? String(vars[name]) : m))
}

/**
 * The language to show for a stored preference: the code itself if there is a file for it,
 * otherwise (and for `"system"`) the best match for the system languages, else English.
 */
export function resolveLanguage(
  pref: string | undefined,
  system: readonly string[] = typeof navigator === 'undefined' ? [] : navigator.languages,
): Lang {
  if (pref && pref !== 'system' && pref in dictionaries) return pref
  for (const tag of system) {
    if (tag in dictionaries) return tag
    const primary = tag.split('-')[0].toLowerCase()
    if (primary in dictionaries) return primary
  }
  return 'en'
}

/** Updates the store and <html lang dir>. Persisting to settings is the caller's job. */
export function setLanguage(lang: Lang): void {
  const code = lang in dictionaries ? lang : 'en'
  langState.lang = code
  const root = document.documentElement
  root.lang = code
  root.dir = LANGUAGES.find((l) => l.code === code)?.dir ?? 'ltr'
}

export function currentLanguage(): Lang {
  return langState.lang
}
