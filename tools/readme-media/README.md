# README media

Re-records the screenshots, GIFs and diagrams in `docs/media`, from the repo root:

```
node tools/readme-media/record.mjs        # English and Arabic
node tools/readme-media/record.mjs ar     # one language
```

It needs Windows, `ffmpeg` on the PATH, `pnpm --dir ui install` and, once, `pnpm --dir ui exec playwright install chromium`. It can run while the dev app is open: it uses its own port and work folder (`target/readme-media`, which git ignores).

## How it works

It runs the real UI in Playwright's own headless Chromium, with the Tauri backend swapped for `demo-backend.js`. Nothing talks to a router or changes your settings. Review the results, then commit `docs/media`.

**Never an installed browser.** Edge or Chrome can bring synced extensions into a new profile, and antivirus flags those files. So this uses only Playwright's Chromium, with `--disable-extensions` and `--disable-sync`, on a fresh profile in the system temp folder. The profile is deleted at the end of every run, including a failed one, and a run fails if it can't be deleted.

**Demo data only.** Every value comes from `demo-backend.js`, which follows the demo driver in `crates/router-core/src/drivers/mock.rs`. Never add real router values, or a list of them, to these files. A capture fails if the page shows a PCI the demo driver doesn't have, or an operator name. That check reads the page text only, so still look at the results.

## What each README uses

`README.md` uses `docs/media/en` and `README.ar.md` uses `docs/media/ar`. A future language's README reuses `docs/media/en`.

GIFs stay under 5 MB (the script fails above that) and last 5 to 10 seconds.
