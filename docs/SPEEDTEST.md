# Speed test

The **Speed test** tab measures the internet connection on the band and cell the router is
using right now, so band sets can be compared on real throughput and latency, not only on
signal values. Besides the router, it and the optional update check are the only network peers. It
runs when you press **Run test**, during an auto-scan you start, and on a schedule only if you
turn on automatic tests in Settings (off by default). Results are never uploaded anywhere.

Code: `src-tauri/src/speedtest/` (`cloudflare.rs`, `ookla.rs`, `mod.rs`, and `schedule.rs` for
automatic tests), UI in
`ui/src/components/SpeedTest.svelte`, `ui/src/components/LastSpeedTest.svelte` and
`ui/src/lib/speedtest.svelte.ts`.

## Rules

- Every request is made by the Rust backend (`reqwest` with rustls). The webview's CSP is
  unchanged and still allows nothing remote.
- A test is refused while a band change is open (safe-apply phase other than idle, committed or
  reverted): the router is re-attaching then and the result would mean nothing.
- A test does not need a router login (it measures the internet, not the router), but it always
  records what was serving at the start, taken from the latest router snapshot: LTE band, PCI
  and EARFCN, whether carrier aggregation was active, and the NR band, PCI and NR-ARFCN.
  Cell IDs and eNB IDs are **not** recorded.
- Only one test runs at a time. **Cancel** stops it at once; a cancelled test is not saved.

## What is measured

| Value | How |
| --- | --- |
| Ping, idle | Median of 10 tiny requests on a warm connection (the first request, which opens the connection, is not counted). The server's own processing time, from Cloudflare's `Server-Timing: cfRequestDuration` header, is subtracted. |
| Download | 4 parallel HTTPS streams, counted as the bytes arrive. The first fifth of the phase (connection set-up, TCP ramp-up) is left out of the rate. |
| Upload | 4 parallel HTTPS streams, counted as the body is handed to the connection in 64 KB pieces, with the same ramp-up rule. |
| Ping while busy | The same tiny request, repeated about every 300 ms on its own connection while the download runs and again while the upload runs. The medians are kept separately ("while downloading" and "while uploading"); the page shows the worse one. A probe with no reply within 5 s counts as 5000 ms. |

Ping while busy (loaded latency, "bufferbloat") is what makes video calls and games lag even
when the download figure looks good. Under 100 ms feels instant; over 300 ms calls stutter. On
the H122-373 with B3/PCI 103 + n41 it was about 1450 ms while uploading, against about 41 ms idle.

The sentence under the title names the worst problem first: a slow upload, then high ping under
load, then a slow download; otherwise it says the connection looks good.

## Data use and time limits

Each transfer stops at its data budget or its time cap, whichever comes first, so a slow link
never runs for long.

| Size | Download | Upload | Download cap | Upload cap | Total |
| --- | --- | --- | --- | --- | --- |
| **Light** (default) | 28 MB | 10 MB | 12 s | 15 s | about 40 MB |
| **Full** | 150 MB | 45 MB | 25 s | 25 s | about 200 MB |

Pings add a few kilobytes. The page shows the data used so far while a test runs. An Ookla test
uses whatever the Ookla CLI decides (typically tens to a few hundred MB on a fast link); the
recorded `bytesUsed` is what the CLI reports.

## Providers and the endpoints they contact

### Cloudflare (built in, the default)

- `GET https://speed.cloudflare.com/__down?bytes=N` for idle ping (`bytes=0`), download and
  ping while busy.
- `POST https://speed.cloudflare.com/__up` for upload (a body of zero bytes).

Cloudflare sees your public IP address and the traffic, as any website does. The data-centre
code from the `cf-ray` header (for example `JED`) is shown as the server. The base URL is
injectable in the code; tests use a local mock server and never the real service.

### Speedtest by Ookla (optional)

The user picked Ookla as their usual tester, so the official **Ookla Speedtest CLI** is offered
as an option. Nothing is downloaded until you pick Ookla **and** accept its terms in the app.

- **Download (first Ookla test only):**
  `https://install.speedtest.net/app/cli/ookla-speedtest-1.2.0-win64.zip`, Ookla's own
  Windows x64 package, fetched directly from Ookla. The app never ships or redistributes the CLI.
- **Running it:** `speedtest.exe --accept-license --accept-gdpr --format=jsonl --progress=yes`, started without
  a console window (`CREATE_NO_WINDOW`), with a 120 s timeout. Cancel kills the process. The CLI
  then talks to Ookla's configuration servers and the nearest Speedtest server it chooses.
- **Parsed values:** `ping.latency` (idle ping), `download.bandwidth` and `upload.bandwidth`
  (bytes per second, divided by 125 000 for Mbps), `download.latency.iqm` and
  `upload.latency.iqm` (ping while busy, the interquartile mean), `download.bytes` +
  `upload.bytes`, and `server.name` with `server.location`. The ISP, IP addresses and the result
  URL in the CLI's output are dropped and never stored.
- **Fallback:** if the CLI can't be downloaded, fails its hash check or can't be started, the same
  test is measured with Cloudflare and the result says so ("Measured with Cloudflare because …"). A
  run that started and then failed or timed out is reported as a failure, so a second test doesn't
  use more data.

#### Ookla's terms (summary, not legal advice)

Read the originals before accepting:

- End User License Agreement: <https://www.speedtest.net/about/eula>
- Privacy Policy: <https://www.speedtest.net/about/privacy>

In plain words, as shown in the app before anything is downloaded:

- The Speedtest CLI is free for **personal, non-commercial use only**.
- Ookla receives your test results and your IP address and handles them under its own privacy
  policy. Read that policy for what else the CLI collects and how Ookla uses it.
- `--accept-license` and `--accept-gdpr` record your acceptance of the licence and of the GDPR
  notice for the CLI; the app passes them only after you accepted in the app. Your acceptance is
  stored as `speedTest.ooklaTermsAccepted` in `settings.json`.

Anyone redistributing this app must not bundle `speedtest.exe`; it is downloaded from Ookla on
request on each PC.

#### Pinned version and hashes

| | Value |
| --- | --- |
| Version | 1.2.0 (the current release on Ookla's CLI page, <https://www.speedtest.net/apps/cli>, when this was written) |
| Zip URL | `https://install.speedtest.net/app/cli/ookla-speedtest-1.2.0-win64.zip` |
| Zip SHA-256 | `13e3d888b845d301a556419e31f14ab9bff57e3f06089ef2fd3bdc9ba6841efa` |
| `speedtest.exe` SHA-256 | `c73c4ac2a7cdc48829fe0f9d7ec91ca41c8b434490d4eb3205aad69c7aa3b921` |

The zip is checked before extraction, the extracted exe is checked before it is written, and the
installed exe is hashed again before **every** run, so a changed file is never executed (the test
falls back to Cloudflare instead and the next Ookla test downloads a fresh copy).

The exe lives in `%APPDATA%\io.github.hjbiki.router-band-tuner\tools\ookla\speedtest.exe`.
Deleting the folder is safe; it is downloaded again on the next Ookla test.

#### Updating the pin

1. Open <https://www.speedtest.net/apps/cli>, choose Windows, and note the version and the exact
   download link of the x64 zip.
2. Download it into an empty temporary folder outside the repo and hash it:
   `certutil -hashfile ookla-speedtest-X.Y.Z-win64.zip SHA256`.
3. Extract it there and hash `speedtest.exe` the same way.
4. Update `VERSION`, `ZIP_URL`, `ZIP_SHA256` and `EXE_SHA256` in
   `src-tauri/src/speedtest/ookla.rs` and the table above. Check the JSON output still has the
   fields listed above (`speedtest.md` inside the zip documents the format).
5. Never commit the zip or the exe.

## Who started a test

Each result and each `speedtest.active` start carries a `source`: `manual` (you pressed Run test),
`scan` (an auto-scan) or `auto` (the schedule). An older record without one is shown as `scan` when
its time falls inside a recorded auto-scan, and as `manual` otherwise; the one-time import into
`history.db` stores the same guess. The History page shapes each test's point by its source, and
Auto-scan says when automatic tests are part of its 7-day averages.

## Automatic tests

Off by default. **Settings › History and automatic tests** turns them on, with:

- **How often:** every 1, 2, 3 (preselected) or 5 hours, or a custom 1 to 168 whole hours.
- **Size and server:** Cloudflare Light unless you choose otherwise. Ookla can be chosen only after
  its terms are accepted on the Speed test page; if they're withdrawn, the schedule falls back to
  Cloudflare.
- **Data use:** the card shows an estimate, the tests per day times the planning figure of one test
  (about 40 MB for Light, 200 MB for Full, 150 MB for Ookla), and a warning above 300 MB a day.
  Every 3 hours on Light is about 320 MB a day.

How the scheduler (`schedule.rs`) behaves:

- It checks every 30 s. The first test comes at least 3 minutes after launch, or after the setting
  is turned on or the interval changed, and each slot gets 0 to 120 s of jitter.
- A test never runs while the app isn't connected to the router, a band change or auto-scan is
  open, or another test runs. A slot that stays blocked for 10 minutes is skipped and logged as
  `speedtest.auto_skipped` with the reason; the next one comes a full interval later. Slots missed
  while the PC slept are never caught up.
- It uses the same guard as a test you start, so while it runs the app holds back Apply, step 2
  of a two-step apply and the re-lock watchdog. Apply or Start scan during an automatic test is
  refused as usual.
- There's no notification; the tray line says "Automatic speed test running". The last attempt
  and its outcome are kept in `auto-speedtest.json`, so the Settings card can show them after a
  restart.

## Where results are stored

Each finished test is stored in `history.db` in the app data folder
(`%APPDATA%\io.github.hjbiki.router-band-tuner\`), with the full result as JSON. It holds the time,
the source, the provider (and the one that was requested, with the fallback reason if any), size,
the serving band, PCI and channel, the allowed band set, idle ping, download, upload, ping while
downloading and while uploading, data used and the server name. The Speed test page and Auto-scan
read the newest 200, or every test of the last 7 days when that's more, so Auto-scan's 7-day
averages see the whole window. Tests are kept for the history retention (90 days by default) and
removed by **Clear history**.

Before M2 they went to `speedtests.jsonl`. That file is imported once at start and then renamed
to `speedtests.jsonl.imported`. If `history.db` can't be opened, can't take the write, or was made
by a newer version of the app (read-only), new tests are appended to `speedtests.jsonl` again: the
pages read that file too, and the next start that can write imports it.

Like `events.jsonl`, these files are **location-sensitive** (band, PCI and channel narrow down the
cell you were on). They stay on this PC, are never uploaded, and must not be committed or posted
publicly.
