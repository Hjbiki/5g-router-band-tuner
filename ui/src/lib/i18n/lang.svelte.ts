// Reactive current language; kept separate so index.ts stays plain TS.
/** A language code with a matching `<code>.json` file in this folder. */
export type Lang = string
export const langState: { lang: Lang } = $state({ lang: 'en' })
