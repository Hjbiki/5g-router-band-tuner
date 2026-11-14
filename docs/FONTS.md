# Fonts

All fonts are bundled in the repository under `ui/src/assets/fonts/` and loaded by `fonts.css` there. Nothing is fetched at runtime (the CSP allows `font-src 'self'` only).

| Use | Font | Files | Licence |
| --- | --- | --- | --- |
| Arabic UI and headings, Arabic text everywhere | IBM Plex Sans Arabic, weights 400, 500, 700 (Arabic and Latin subsets) | `ibm-plex-sans-arabic-*.woff2` | SIL Open Font License 1.1, © 2019 IBM Corp. (`OFL-IBM-Plex-Sans-Arabic.txt`) |
| Latin text and figures | Geist, variable weight (Latin and Latin Extended subsets) | `geist-*-wght-normal.woff2` | SIL Open Font License 1.1, © 2024 The Geist Project Authors (`OFL-Geist.txt`) |

How they are used (`ui/src/app.css`):

- `--font-ui`: Geist first, then IBM Plex Sans Arabic for any Arabic glyph. Used for English UI and headings.
- `--font-ui-ar`: IBM Plex Sans Arabic first. With `<html lang="ar">` it sets the whole UI and the headings, so Latin words and figures inside Arabic lines match the Arabic.

## Licence notes

The SIL Open Font License allows using, bundling, embedding and redistributing the fonts, including in this repository and in the app's installers, as long as:

- the licence text stays with the font files (it does: the two `OFL-*.txt` files);
- the fonts aren't sold on their own;
- modified versions don't use the reserved font names.

## Updating

The files were copied unchanged from the `@fontsource/ibm-plex-sans-arabic` (5.3.0) and `@fontsource-variable/geist` (5.3.0) npm packages, keeping their unicode ranges. To update them, take the same files from newer versions of those packages and copy the `@font-face` blocks for the subsets listed above.

## History

Thmanyah Sans was tried for headings and Arabic text and dropped on 2026-10-30. Its licence forbids redistributing the font files, including in a public repository, so it can't be part of an app whose source is public. It was never committed to the repository.
