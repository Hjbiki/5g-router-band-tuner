# How it works

The details behind the README: the safety model, security and plain HTTP. Protocol details are in [API_NOTES.md](API_NOTES.md).

## Safety model

Every configuration change goes through the same pipeline:

1. **Validate** the requested configuration against what the router reports as supported.
2. **Snapshot** the current configuration.
3. **Journal** the intent to disk (`pending-change.json` in the app data folder) before touching the router.
4. **Apply** the change.
5. **Verify** by reading the configuration back.
6. **Re-attach grace** (90 s by default) while the router re-registers on the network.
7. **Probation** (3 min by default). If service is lost for longer than the loss threshold (60 s by default), the change is reverted. You can press "Keep now" to accept early or "Revert now" to undo early.
8. **Revert**, and if the revert itself cannot restore service, **fall back to Auto**.

The journal means this survives app crashes, restarts and the computer sleeping: on the next start the app resumes the change and finishes or reverts it. When a password is saved (or the demo driver is selected), the app connects by itself at launch, so a resumed change is watched even if you never open the window. Without a saved password the change stays pending until you save one and press Connect. If the router session is lost or taken over (for example by someone logging in on the router web page), the app logs in again on its own and reverts as soon as it can; while the web page holds the login it asks you to close that page.

What the app will never do:

- Shut the router down. That code path does not exist.
- Factory reset the router on a model where the exact call isn't verified. There the button stays off and nothing is sent.
- Offer a factory reset by itself. Where it is verified, the red **Factory reset…** button beside Restart opens a short warning, and the reset is sent only when you press its confirm button within 30 seconds. It is off during a band change, a scan or a speed test.
- Reboot without a two-step confirmation (and not while a change is in progress).
- Retry a rejected login automatically. A wrong password, an unrecognised login error from the router, or a login algorithm mismatch stops the app, because retrying could lock the account. A wrong password, a lockout or an unrecognised error also blocks the saved password until you save it again; a refused security token (125001 to 125003) doesn't, because it comes before any password check.
- Send a login proof by itself to a router it doesn't recognise. At launch, it logs in only if the router gives the same login salt and iteration count as at the last login whose server signature verified. Otherwise it stops before the proof, tells you, and asks "Is this your router?". It also refuses a router asking for fewer than 1000 iterations.

  This narrows the risk and doesn't remove it. A device that has watched the real router's login on the same network could repeat its salt. And once a run has verified the router, later logins in that run don't ask, so a band change can always log back in to revert; a PC that moves to another network with the app still running can still send a proof.

## Refresh IP

**Refresh IP** on the Network page asks the network for a new internet address. Each driver says which ways its router allows, in order (`Capabilities.ip_methods`), and a run uses the first one the router hasn't refused since the last Connect.

- **Network search (the H122).** The app sends `GET /api/net/plmn-list`, the router's manual search for mobile networks, as a third-party bookmarklet does. The router drops its data connection while it searches and reconnects by itself, usually with a new address. The search runs on its own task with a 180 s limit, so the app keeps watching the router meanwhile, and the session is renewed first if it's older than a minute. Only the number of networks found is kept. A search can't be stopped once sent: Stop ends the run after it. Then the app waits up to 150 s for service and an address, read every 2 s starting 10 s after the router answered. One search counts as one try in the same budget as below, with at least 20 s between tries and a 20-minute cap. Afterwards the band lock is read once; if it changed, the app logs it and offers **Restore my bands** (a normal safe apply), and changes nothing by itself.
- **When the internet doesn't come back** in time, the banner says so in plain words (the router usually reconnects by itself) and offers **Restart router**, which opens the usual two-step reboot confirmation. A reboot is never automatic.
- **Band switch for a new IP.** When a search kept the same range or was refused, the banner offers **Try a band switch**, never automatically. It's a two-step band change through the same safe apply and sequence file as the primary band: first the serving band alone (or every supported band when only one band is allowed), kept as soon as it has an address, then the bands from before, with the full trial and automatic undo. If the temporary bands get no connection, the change is undone; if the bands from before don't come back with one, the router stays on the temporary bands, which worked. At most 3 an hour, refused while anything else holds the router, and refused while "Undo a change if the internet drops" is off, since without it a step with no connection would be kept.
- **Mobile data off and on** is the other way, for routers that accept it; it's described below. If a router refuses the "off" (as the H122 did on 2026-11-07) and the search is on its list, the same try goes on as a search, and the refusal isn't treated as a lost session.

The data-switch method, step by step:

- **The note comes first.** Before the "off" request, the app writes `pending-data-switch.json` to the app data folder and flushes it to disk. If that write fails, nothing is sent.
- **Data always comes back on.** After the off time (5, 10, 20, 30 and 30 s by try), the app sends "on" and reads the switch until it says on. Stop, a lost session or the 12-minute cap turns data on at once. A failed "on" is retried every 5 s, then every 30 s, with an alert. The note is cleared only once the switch reads on.
- **Across a restart.** At launch the note is read before the band journal. If the app was closed with data off, it turns data back on right after the first login. A note for another router, or one that can't be read, is shown and never acted on.
- **Data off without a note** (turned off on the router's own page, say) only gets a notice and a **Turn mobile data on** button. The app never turns it on by itself.
- **Limits.** A target is one or two numbers (`10` or `10.45`); 1 to 5 tries; at least 10 s between tries; at most 15 off/on cycles an hour. A reboot is never used as a fallback.
- **One thing at a time.** A Refresh IP run can't start during a band change, a scan, a speed test, an APN change or an open reboot confirmation, and those wait or are refused while it runs.

A "work" connection with more data usually comes from a different APN or plan, not from an address range, so the app says so when a run finds no match.

## Log out

**Log out** (Settings, router connection) ends the app's own session on the router. It's refused while a band change, an APN change, a scan, a speed test or an IP refresh holds the router.

- The `logged-out` marker is written first, then the router is asked to end the session (5 s at most). The app forgets its cookies and tokens and everything it read, so no old band chips stay on screen, and nothing reaches the router until you press **Connect**.
- It ends logged out even when the router didn't answer, with a note saying so; the router's session then simply expires.
- The saved password and the router identity stay, so Connect needs no typing and doesn't ask "Is this your router?".
- **Staying logged out across a restart.** With the marker set, the launch auto-connect stays logged out, unless something needs watching: an open band change, primary-band sequence or APN change, a journal for another router or one that can't be read, or an IP refresh to finish. Then it logs in anyway and logs that it did. Pressing Connect clears the marker.

## APN profiles

The Network page lists the router's APN profiles, marks the one in use and the ones the operator set, and can switch, add, edit and delete them.

- **Switching** goes through the same safe apply as a band change, with its own journal (`pending-apn-change.json`, archived to `journal-archive-apn/`) and fixed timers: 90 s to come back, then 2 minutes of trial, with 60 s of lost service enough to undo it. Service needs an internet address too. There is no fallback like Auto for bands: if the previous profile can't be restored, the change ends as "revert failed", is retried every 30 s and raises an alert. The auto-undo setting doesn't apply, so a profile without data is always undone. Switching is off while the router's own Auto APN is on.
- **Add, edit and delete** are only for profiles that aren't in use, aren't the operator's and are complete. Each write is checked against a fresh read before and after; if the router dials with another profile afterwards, the app switches back with automatic undo and tells you.
- **One change at a time.** While an APN change is open, band changes, the primary band's step 2, the re-lock watchdog, logout, Refresh IP, a reboot and a factory reset are refused or wait, and automatic speed tests skip.
- **Across a restart** the APN journal is resumed after the first login, like the band journal. A record for another router blocks APN changes only.
- **Passwords.** A profile's saved password never leaves the driver: the app keeps only whether there is one. A password you type is sent to the router in that one request and nowhere else. It's never written to settings, logs, events, history, the diagnostics export or an error message. Events name a profile by its index only, never by name or APN.

## Tray icon

The tray icon keeps watching and reverting while the window is closed, with OS notifications. Grey: not configured and no change being watched. Green: in service and idle. Amber: re-attach grace, probation, reverting, reconnecting, or the session was taken over. Red: no service, revert pending, or revert failed. The red states win over grey, so a change resumed without a saved password still shows red.

## Router page scripts

**About › Router page scripts** saves the router's own web page code to a zip file on this PC, so a call such as the factory reset (and later the APN changes) can be checked in the firmware instead of being sent to find out. It only reads.

- **When.** Only while the app is logged in, and not during a band change, Refresh IP, band switch for a new IP, APN change, auto-scan or speed test. The actor grants the read itself (`BeginPageScripts`) and, until it ends, refuses band changes, Refresh IP, APN changes, reboots and logouts, holds back a primary band's step 2 and the re-lock watchdog, and no speed test or auto-scan starts. Turning mobile data back on is never held up, and a safe-apply undo is never blocked.
- **Where it goes.** The zip is written only where you choose on this PC; nothing is sent anywhere else.
- **GETs only.** It starts at `/html/content.html` with the app's session and follows what that page links to, breadth first to depth 3. Each file is a plain GET with the session cookie, without the API headers or a token, never following a redirect (a redirect means the session is gone), 5 s and 2 MB at most.
- **Which paths.** A reference is cut at `?` or `#`, percent-decoded once, and must then hold only letters, digits, `_`, `.`, `/` and `-`, with no `..`, `//` or backslash. It must resolve to the same router and lie under `/js/`, `/lib/` or `/html/`, ending `.js` or `.html`. Anything else, including other hosts, is listed as left out and never fetched. The driver checks the path again before each request.
- **Limits.** 400 files, 8 MB in all, 3 minutes, 50 ms between requests. A gzip body (recognised by its first two bytes, whatever the headers say) is unpacked up to 4 MB. A short HTML answer where a script was asked is the router's error page and counts as missing.
- **Redaction.** CSRF token values (`<meta name="csrf_token">` and any `__RequestVerificationToken` value) are blanked. Every token the diagnostics privacy check would flag (IMEI- and ICCID-length numbers, IP and MAC addresses) is replaced with `[redacted-<kind>]`. A file the final check, which also looks behind `\n` escapes, still flags is left out.
- **Review, then save.** The summary (files, size, what was missing or left out) comes first; **Save zip…** then asks where to write `router-page-scripts.zip`, with a `MANIFEST.txt` of the app version, driver, firmware version and each file's size, gzip flag and redaction count. The save is logged with counts only.
- **Keep it private.** The zip is the router maker's code. Don't commit it (`router-page-scripts*.zip` is git-ignored) or post it publicly.

## Record how a router works

**About › Record** (and the notice for a router the app doesn't support) opens the router's own web page in a window of its own and records what that page asks the router. The cleaned file tells a developer how that model logs in and what it answers, which is what a new driver is written from. Nothing is sent anywhere: you save the file yourself.

- **When.** Only while the app itself isn't talking to the router, so on a supported router you log out of the app first. Not during a band change, auto-scan, speed test or page-script save, and not with the demo router. While a recording is open or waiting for its review, the app won't connect.
- **The window is sealed.** It gets no permission in the app, so the router's page can call none of the app's commands. Its browser is started with a proxy that can't be reached and only the router's own address goes around it, so a frame, a picture, a redirect or a WebSocket to anywhere else never leaves this PC. A proxy carries no UDP, so calls set up by a page (WebRTC) are kept to the proxy and QUIC is off. The window doesn't listen to messages from the page. It has its own browser profile, made for this recording and deleted after it. It can open no other window, download nothing and get no permission. If any part of this can't be set up, the page isn't opened.
- **What you do.** A box on the window gives the three steps: log in, wait, close. The window's title changes to **Done** once the page has logged in and stopped asking for new things, so nobody has to count seconds. Closing the window, by its Stop item or its X, ends the recording and brings up the review.
- **What is recorded.** Only requests to the router's own address: the path, the names of headers (never their values), what was sent, and the answer. Up to 500 requests, 2 MB an answer, 20 MB and 15 minutes in all. It lives in memory only. An answer with far more values than a router sends, or nested far deeper, is left out of the file and says so.
- **How it is cleaned.** Stricter than the diagnostics export, because a router's page can show anything. Everything a login sends is hidden. A value stays as it was only under a name known to be technical (signal, bands, frequency, mode, state, version, model, operator), and only when it reads like a reading or a state and holds no identifier. Everything else is hidden or replaced by a made-up value of the same shape. Field names that look like data are replaced too. A page's markup is never kept, and the router's script files only if you tick the box. The finished file is scanned for IMEI- and ICCID-length numbers, IP and MAC addresses, and for anything that was hidden elsewhere; a hit means nothing can be saved.
- **Review, then save.** The review lists the only values still readable in the file, which is short enough to actually read, above the file itself. **Save file…** writes exactly that text where you choose. Discard, or quitting the app, throws the recording away.
- **What it can't tell.** Most routers' own pages have no band setting, so a recording shows who the router is, how it logs in and what it reads, not how to change bands. That comes from the router's script files or from what is known of that maker.

## Privacy check

**Router › Privacy check** shows what your operator can reach on the router and which devices are on your network. It only reads, and only when you press its button.

- **What it reads from the router.** Two things, with the app's session, GETs only: the remote-management (TR-069) settings (`/api/cwmp/basic-info`) and the device list (`/api/lan/HostInfo`, or `/api/wlan/host-list` when the router won't give the first). Three requests at most per press. Nothing is polled, and neither read is part of the diagnostics export.
- **When.** While logged in, one check at a time. A check isn't started during a band change, Refresh IP, APN change, auto-scan or speed test, or while the page scripts are being read. One of those that begins while a check is still reading isn't held back: the check's reads are plain GETs that don't use the router's tokens.
- **A refused read is shown as refused.** Each read ends by itself: "The router refused this read (code N)", "This router has no page for this (code N)", "The router didn't answer". The code is the router's own, or the HTTP status when the router answered with one (404 reads as "no page"). A refusal here doesn't count as a lost session, and the other read still runs.
- **Remote management.** The app shows whether a management server is set and its host name. Only a field that is named as an address, or that holds an `http://` or `https://` address, counts: a switch or a port named for the server never reads as "a server is set". It never keeps the account, the port, the path or the router's own connection-request address. It says whether the feature is switched on only when the router's answer has the field named exactly `enable`, as the H122's does; otherwise it says it can't tell. TR-069 means the operator *can* read and change the router's settings; it doesn't show them the sites you visit.
- **Devices.** The card says which list the router gave: every device it remembers, online or not, or only the ones on Wi-Fi right now. Names, MAC addresses and local addresses appear only after **Show devices**, and only on that screen. A device's name is whatever the device calls itself, so it's cleaned (no control or text-direction characters, 64 characters at most) and shown as plain text. A private (randomised) Wi-Fi address is recognised from the address itself. Each device gets a picture guessed from its name, a Wi-Fi or cable mark from the router's own field, and "This PC" for the computer the app runs on.
- **Reachable from the internet** uses the address type the app already knows (Router › Internet address). No new request. That type is about IPv4 only: when the router also has an IPv6 address, the card says so and gives no assurance about it, because the app doesn't read the router's firewall.
- **This PC's DNS server** is the only step that leaves your network: one ordinary lookup of `whoami.akamai.net` through Windows, which answers with the address of the DNS server that asked. It runs when you press **Check DNS**, shows the address as its first two numbers until you ask for all of it, and isn't logged. It doesn't name the company that runs the server; that would need a second request to a public registry. The demo driver looks nothing up and shows a made-up address. Windows may keep the answer for a few minutes, so wait before checking again after changing your DNS settings.
- **What is kept.** Nothing that was read is written to disk. `events.jsonl` gets one `privacy.checked` line with, for each read, how it ended, the router's code, the element names of the answer and which device list answered: never a value, an address, a name or a count. The element names are the router's own words, so one shaped like an address is left out. The result stays in memory while the app runs, so leaving the page keeps it; logging out of the router, changing the router in Settings (an answer still on its way then is dropped too) or closing the app forgets it.
- **Seen on the H122 (2026-11-13).** Both reads answered and the session stayed. `wlan/host-list` wasn't needed, so it is still unconfirmed.

## Privacy details

- Local only. The network peers are your router (default `192.168.8.1`, including a read of its public pages to recognise an unsupported model), the speed test when you press **Run test** or start an auto-scan, automatic speed tests if you turn them on (off by default), GitHub for the update check if you allow it, and one DNS lookup when you press **Check DNS** in the privacy check. The webview loads bundled assets only and the CSP allows nothing remote.
- The speed test runs only on request: `speed.cloudflare.com` by default, or, if you choose Speedtest by Ookla and accept its terms, Ookla's official CLI, downloaded from `install.speedtest.net` on first use and then talking to Ookla's servers (Ookla receives your results and IP address). All of its requests are made by the Rust backend. Results go to `history.db` in the app data folder with the band and PCI they ran on, so keep that file private too. Details: [SPEEDTEST.md](SPEEDTEST.md).
- The router password is stored in the Windows Credential Manager. It is never written to settings, logs or crash output.
- No telemetry. The update check is off unless you allow it, runs at most once a day, and only reads the latest release from GitHub. Nothing is downloaded or installed.
- The app keeps a local event log, `events.jsonl`, in the app data folder. It contains PCIs, and cell IDs in entries written before the History version, which are location-sensitive. **Do not post it publicly.** For a bug report use **About › Export diagnostics…** instead: it replaces serial numbers, IMEI, MAC and IP addresses and cell identifiers with made-up values, shows you the result, and refuses to save if its privacy check still finds an identifier. Nothing is uploaded. Keep `band-runner` results in `band-runs/` private for the same reason.
- The History page reads `history.db`, a SQLite file in the app data folder. While the app runs it records the signal once a minute, every speed test, and a copy of every logged event. It's kept for 90 days by default (Settings can choose 30, 365, another number of days or forever) and **Clear history** empties it. It's never uploaded and isn't part of the diagnostics export.
- The History page opens on one answer: which band set has been fastest here, once a set has at least 3 tests and a 20% lead. Below it are short cards for the signal, speeds and automatic tests, and the charts and tables sit in **More details** sections that start closed and remember, on this PC only, which ones you opened: signal over time, all speed tests, speed by time of day, speed by bands, speed by IP range, and what happened. The app's own band changes, scans, IP refreshes and APN changes never count as lost service. New users start on the last 7 days.
- **IP address privacy.** The full internet address stays in memory and on the Network page, behind a **Show full** toggle that isn't saved. Only its first two numbers (the `/16`, such as `10.45`) are ever written: with speed tests, in the history's signal minutes and in events. Nothing about IPv6 is stored beyond whether there is one. The CSV export includes the range only when you tick "Include the IP range" for that export, and the event log drops keys such as `ip`, `wan_ip`, `ipv4` and `wanipaddress` at any depth.

## Security

- **No listening sockets.** The release build opens no TCP or UDP listener (checked at runtime, app and WebView2 processes). Its connections are outbound: HTTP to the router, plus HTTPS for a speed test you run, automatic speed tests if you turn them on, and an update check you allow. The privacy check's **Check DNS** adds one name lookup through Windows, when you press it.
- **Locked-down webview.** The production CSP allows only bundled scripts and styles (`script-src 'self'; style-src 'self'`), no plugins, frames, forms or `<base>` (`object-src`, `frame-src`, `form-action`, `base-uri` and `frame-ancestors` are `'none'`), and no network access except Tauri IPC. The window's Tauri permissions are limited to listening for events, reading the app version, and what its own title bar needs (start dragging, minimise, maximise or restore, check whether it is maximised, and close, which Rust turns into hide-to-tray). It can't resize, move, create or reconfigure windows any other way. Notifications, dialogs, tray and autostart are driven from Rust. The only link that leaves the app (the developer page on the About tab) is a fixed URL opened from Rust; the webview has no opener permission.
- **Write-only password.** No command returns the saved password, and the HTTP client never follows redirects, so the CSRF token can't leak to another host.
- **Rejected passwords aren't retried.** After the router rejects the saved password or reports a lockout, the app won't send that password again until you save it again.
- **What the history holds.** `history.db` stores the band, PCI, EARFCN, NR-ARFCN, the operator name and the allowed band set, with the signal values. It never stores cell IDs, eNB IDs, sectors, TACs, IMEI, IMSI, ICCID, serial numbers, IP or MAC addresses: the signal recorder doesn't read them, and each event is cleaned of those keys, at any depth, before it's copied in. Cell changes in `events.jsonl` no longer carry the cell ID either, only the network type, PCI and band.
- **CSV export.** **History › Export CSV…** leaves out the PCI, EARFCN and NR-ARFCN columns unless you tick "Include cell numbers" for that export; the box isn't remembered. Band numbers are always included, cell, eNB and TAC IDs can't be exported at all, and free-text error messages are left out. Server names are cleaned, and a final scan of the whole file stops the save if it still finds an identifier. Cells that start with `=`, `+`, `-`, `@`, a tab or a carriage return get a leading `'`, so Excel doesn't run them as formulas.
- **Automatic speed tests** are a third use of the network besides the router and the update check, and they're off by default. When on, they contact only the speed-test server chosen for them (Cloudflare, or Ookla once its terms are accepted).
- **Dependencies** are checked with `cargo audit` and `pnpm audit`. The remaining `cargo audit` warnings come from Tauri's own dependency tree.

## Plain HTTP

The router only speaks plain HTTP, so traffic between this app and the router is not encrypted. The login uses a challenge-response scheme so the password isn't sent in the clear, but everything else is visible to anyone on the same network. Use the app only on a trusted LAN.

## Using it on a real router

Read [DEVICE_CHECKLIST.md](DEVICE_CHECKLIST.md) before your first change. It walks through the read-only checks and a safe first apply, and says how to report what you see.
