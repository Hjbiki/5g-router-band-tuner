# Real-device checklist (H122-373, firmware 11.0.2.21)

Run this once on the real router, in order. Tick each box and note what you see. At the end, record the results in `docs/API_NOTES.md` section 7 (step 11). Do not paste passwords, tokens, cell IDs, serial numbers or addresses anywhere public.

Before you start: enter the router host and username in Settings, set the router driver to **Huawei H122**, and save the password (it is write-only and goes to Windows Credential Manager). Be on a stable link, and expect short internet drops during steps 5 to 7.

## 1. Read-only first connect
- [x] Connect. Login succeeds and the server signature verifies (see the event log). *2026-10-29: pass, after fixing a 125003 on the first login of a fresh process (see API_NOTES §7).*
- [x] If "Login algorithm mismatch" appears, **stop and report it.** *2026-10-29: did not appear.* The app never retries, and you shouldn't either.

## 2. Header case
The app's HTTP client sends header names in lowercase; the browser used mixed case. Confirm POSTs work.
- [x] A change request (step 5) is accepted by the router. *2026-10-29: pass (applied and verified, no session or token error).*
- [x] If every change fails with a session error, report it. *2026-10-29: did not happen; lowercase header names are accepted.* Header case is the first suspect.

## 3. Fresh-session check
- [x] Quit the app completely, start it again and confirm it performs a real login (the event log shows a login), rather than skipping it because state-login reported "already logged in". *2026-10-29: pass (full login of about 740 ms after a fresh start).*

## 4. Band list
- [x] The band list is parsed and band editing is enabled.
- [x] The supported bands shown match B1, 3, 5, 7, 8, 20, 28, 32, 34, 38, 39, 40, 41, 42, 43.
- [x] Live signal, status and cell info show plausible values (n41 for NR-ARFCN 528980, not n7). *2026-10-29: pass. The header showed only "LTE (4G)" during CA + NSA because the router reports code 101; it is now built from the live signal.*

## 5. Harmless apply
- [x] Select the current bands plus one extra supported band. Check the preview, then Apply safely. *2026-10-29: B7 added (8080005 → 8080045), accepted.*
- [x] Watch the re-attach grace countdown, then probation.
- [x] Press "Keep now". The change is committed. *2026-10-29 re-test: pass ("Keeping…" then "Band change kept"). First run FAILED: Keep/Revert seemed dead while the router was slow during re-attach; one Revert landed after about 50 s. Fixed (commands now interrupt routine polls; the buttons show "Keeping…"/"Reverting…"). Re-test.*

## 6. Revert now
- [x] Apply a harmless change again.
- [x] Press "Revert now" (in the window, or from the tray menu). The previous configuration is restored and verified. *2026-10-29: pass (22:08:55, user-requested revert to 8080005, read-back confirmed). Tray-menu Revert not exercised.*

## 7. Auto-revert with the window closed
Warning: your internet will drop for a few minutes.
- [x] Apply a band set with no coverage (for example a single band your operator does not use nearby). *2026-10-29: B43 only.*
- [x] Close the window to the tray.
- [x] The tray turns red while the change has no service (amber only while a change is in service, so a band with no coverage goes straight to red). *2026-10-29: red, then green after the revert.*
- [x] Within about 2.5 minutes with the default timers (90 s grace + 60 s loss) the app reverts automatically and shows an OS notification. *2026-10-29: reverted 97 s after the apply (no re-attach), outage about 1 min 41 s. OS notification NOT confirmed (not noticed; send result isn't logged; dev builds may lack a Windows app identity). Post-merge item.*
- [x] Service is back and the window shows the revert result. *2026-10-29: back on B3/PCI 103 + n41.*
- [ ] Installed (bundled) build only: trigger a revert, see the toast, and find a `notify` line with `"key": "notify.reverted"` and `"appId": true` in `events.jsonl`. `"result": "queued"` only means the plugin took it, so the toast itself is the proof. A dev or `--no-bundle` build logs `"appId": false`.

## 8. Session takeover
- [x] With the app running, open the router web UI in a browser and log in. *2026-10-29: NO takeover. Browser and app stayed logged in at the same time and the app kept working: this firmware allows at least two concurrent admin sessions.*
- [x] Record the error code the app shows (expected 108003): *none; 108003 never appeared.*
- [ ] The banner text tells you another session took over. *Not reachable on this firmware (no takeover).*
- [ ] Close the browser tab (or log out there). The app reconnects. *Not reachable on this firmware.*
- [ ] Data requests after the takeover: the router answers `<error><code>100003</code>` ("no rights"). The app must show "taken over" (or "reconnecting") and log in again by itself once the tab is closed, with no Connect click. If it sits there instead, report it: this is the C1 path (100003 → session lost → re-login).

## Deferred to after the M1 merge (2026-10-29)
Steps 8a, 9 and 10 (except the items marked as passed), the tray-menu "Revert now", and the OS notification on revert (each attempt is now logged as a `notify` line; check with an installed build, step 7). The 100003 → re-login path is covered by tests, and by the 5-minute session expiry seen on the device, but not by a takeover.

## 8a. Signal mode values
- [ ] Record the `<mode>` value `/api/device/signal` reports in each network state: 4G only ________, NSA (LTE + NR) ________, SA (NR only) ________.

## 8b. First poll after an apply
- [x] Right after "Apply safely" (step 5), watch the first few polls: does the dashboard still show the old cell/band for a poll or two before the router drops and re-attaches? Note how many seconds: ________. *2026-10-29: "re-attached" was declared 3.7 s after verify, before the new cell (B3/PCI 103 → B20/PCI 120) was confirmed, so the grace window ended on a reading that was probably stale. Fixed in 58627ba: re-attach now needs a 10 s settle window on fresh polls. Re-test 2026-10-29: pass (settle, then probation).* (If the old cell keeps showing, grace may be cut short by a stale "in service" reading; report it.)

## 9. Unauthenticated GETs
- [ ] Log out in the app and note which GET endpoints still succeed without a session (state-login, token, signal, status, net-mode, and so on): ________

## 10. GUI items
Use the real device for these where possible; the **Demo (no router)** driver (password `mock`) works for the rest.
- [ ] Tray colours: grey when not configured and idle (a pending or failed revert is red even when not configured); green when in service and idle; amber during re-attach grace, probation, reverting, reconnecting or session takeover; red when there is no service, a revert is pending, or a revert failed.
- [ ] Closing the window hides it to the tray, with the notice shown only the first time.
- [ ] Quitting during a change asks for confirmation, and relaunching resumes the change and connects by itself (password saved), with the window still closed.
- [ ] Arabic: layout mirrors (RTL) on all four tabs; band chips and preview stay LTR; `-95 dBm` renders in the right order; countdowns use Arabic-Indic digits when that toggle is on.
- [ ] Dark mode follows the OS; chips, segmented control, banner and modal look right.
- [ ] *(2026-10-29: "Keeping…"/"Reverting…" feedback and band edits surviving a tab switch: pass.)* Info toasts pause on hover or focus, and error toasts stay until dismissed; the session banner appears on takeover; sparklines show gaps during re-attach.
- [x] Reboot from About: two-step flow, 30 s countdown that expires and closes, Confirm disabled during an active change. *2026-10-29: two-step confirm and 30 s countdown worked; the app reconnected by itself after the 60 s wait and the router came back on B3/PCI 103 + n41. (Countdown expiry and the active-change guard not exercised.)*
- [ ] Settings: the password field is empty after saving; changing host during an active change shows a message; timer range errors show.
- [ ] Foreign-journal notice: a `pending-change.json` left by a different host shows the notice, blocks editing, and "Switch back" works.
- [ ] Presets save and delete; the Auto preset works; an empty band mask is blocked; NSA with 5G-only shows a warning.

## 11. Reporting
- [ ] Add what you observed (error code for step 8, GETs from step 9, header-case outcome, anything unexpected) to `docs/API_NOTES.md` section 7, with no secrets or identifying values, and open a pull request or issue.

## Auto-scan and Ookla live progress

A full Quick scan ran on the real router on 2026-10-31 (15 configurations): it finished, each band with no service reverted correctly, and the band sets that worked were measured. Cloudflare and Ookla both ran on the real router with live numbers.

On 2026-11-01 an unattended runner drove the release build through its own UI on the real router, while the user was away. It used Quick and Cloudflare Light with 1 test each. Results below; the ticks with a date come from that run.

- [x] A Quick scan finishes, ranks the results, and ends on the starting bands.
- [x] A band the operator doesn't serve is marked "No service" and the scan moves on.
- [x] A band with no cell gives up about 30 s after the request. *2026-11-01, B5 twice: give-up at about 31 s both times, previous bands back 38 s and 53 s after the start (about 100 s before this change). In the 53 s run the router was too slow to take the revert for 13 s, so the app waited as designed.* Bands that worked all re-attached in 15 to 25 s.
- [x] B1 alone is really tried. *2026-11-01: request accepted, measured 63.5 down, 3.3 up, with n41. The lost session on a band request didn't come back in this run.*
- [ ] Each chip tints as soon as its result is in. Screenshots show dead bands tinted, but the moment of tinting wasn't watched.
- [x] The summary above the results reads correctly. *2026-11-01, Arabic UI.*
- [ ] 2 or 3 tests per configuration show the average and the spread. Not run: these tests used 1 each.
- [x] Stop during settling and during testing goes back to the starting bands and keeps the results. Resume carries on from the band it stopped on, and Restart runs everything again. *2026-11-01.*
- [ ] Stop during applying. Not run.
- [ ] Closing the app mid-scan: the next launch restores the starting bands, and Resume runs the rest.
- [ ] Unplugging the router's internet or power mid-scan: the scan recovers or reports it, and never stays on a test band.
- [ ] 5G is marked correctly, and the times and data use are near the estimate.
- [x] Ookla shows live numbers during its test (`--format=jsonl --progress=yes`).
- [x] A Cloudflare test finishes on the real router.
- [x] A Light test takes about 30 s at most. *2026-11-01: 14 s, on an idle link.*
- [ ] The same right after another test, which is when it took 58 s. Not run.
- [ ] Results: a scan where a speed test fails says there isn't enough data, or recommends from recent tests of the same bands and says so. Not seen: no test failed on 2026-11-01. Unit tests only.
- [x] A star on the same bands can be applied, and runs in two steps. *2026-11-01, B3 with B1+B3: B3 alone, then B1+B3, each kept after its 3-minute trial, ending served on B3 with n41. The internet dropped for 1 to 3 s at each step.*
- [x] The Speed test page: start button at the top, and the numbers stay in place during a test. *2026-11-01.*
- [ ] 10 results a page, on the real page. Unit tests only.
- [x] `EndcStatus` and `endcRestrictedStatus` are read from the real router. *2026-11-01: 1 and 0 whenever 5G was up, and `EndcStatus` 0 while there was no cell at all.*
- [ ] The plain-words reason for missing 5G on the 5G card and in scan results. It never showed, because every band that worked had 5G. Its wording rests on another project's notes on these fields until it's seen on a real no-5G cell.


## Router identity check (built 2026-11-01, not yet tried on the router)

- [x] First launch asks "Is this your router?" and logs in only after "Yes, log in" (2026-11-01, seen in `events.jsonl`).
- [ ] **The salt stays the same:** quit the app, wait 6 minutes so the router's session expires, and start it again. Do it twice. It should log in by itself with no question. A restart inside those minutes finds the session still open and proves nothing. If it asks every time, this router's salt changes between logins and the check needs another stable value.
- [ ] A session lost mid-run, and a band change's revert, log back in without asking.
- [ ] After Remove saved password (Settings), then typing it and pressing Save and connect, it logs in without asking.

## Password and login panel (built 2026-11-06, not yet tried on the router)

Don't type a wrong password on the router to test this unless the user chooses to: each wrong try brings its login lock closer.

- [ ] First run: step 2 of the guide waits on "Checking the password with the router…", says it works, then moves on by itself.
- [ ] Skip for now with an empty field: the guide goes on, and a neutral "Enter your router password" card shows above every page.
- [ ] "Is this your router?" shows above every page, not only Signal. Not now hides it until the question goes away; Settings still asks.
- [ ] Settings > Router connection: Save and connect logs in in one step, Change password and Remove saved password work, and Save changes never mentions the password.
- [ ] Editing the username or address in Settings disables the password field until Save changes, and then says a password is needed if none is saved for that account.
- [ ] The sidebar status reads in a few words ("Password needed", "Connecting…").

## Auto-scan page and band chip popover (built 2026-11-01, not yet tried on the router)

- [ ] The Auto-scan sidebar item opens setup, then live progress, then results. The Bands page has no scan on it, and a running scan shows a dot on the sidebar item and a link on the Bands page.
- [ ] The three steps: a running scan stays on Scanning, a finished one opens on Results, and Back to setup and Back to results work.
- [ ] After a real scan with earlier tests, the summary's numbers match row 1 of the table, with Today beside the 7-day average.
- [ ] A band chip's i shows the real last result: date and time, speeds, ping, 5G, serving cell, and Auto-scan or your own test. It still opens during a change or a scan.
- [ ] After a real scan, a no-service band's chip says "Auto-scan"; after a manual lock it says "Your own test or change".
- [ ] B32 alone is never tried in Quick. In Thorough it's only tried added to bands that worked, and it isn't tinted "no service" from an older result.

## Combinations and band colours on the Auto-scan page (built 2026-11-06, not yet tried on the router)

- [ ] Setup lists the bands under FDD, TDD and SDL headings, each chip with its colour stripe, and the Bands page groups them the same way. Check the stripe next to a green "good" chip in each palette, light and dark, and in the release build.
- [ ] A Quick scan with one added combination (for example B1 + B3 off the current set, or B3 + B41) tries it after the single bands, and its row says "your combination".
- [ ] A combinations-only scan (all single bands unticked) runs the current set and the combinations, and ends back on the starting bands.
- [ ] A combination with no service reverts by itself like a single band, and the scan moves on.
- [ ] After a Quick scan, "Test combinations of the bands that worked" opens Setup with those combinations and no single bands, and Run again repeats the same combinations.
- [ ] The time estimate grows with each combination added; note how long a real one takes.

## History and automatic speed tests (built 2026-11-06, not yet tried on the router)

- [ ] A day of tray-only recording: close the window to the tray for a day, then open History. The signal chart has no gap for that day, and the summary's time in service looks right.
- [ ] Turn on automatic tests every 1 h, then start a band change just before a slot. The test doesn't run during the change, the Settings card says it's waiting, and if the change lasts past 10 minutes the slot is skipped and the event log says why. The next test comes a full hour later.
- [ ] An automatic test that does run shows "Automatic speed test running" in the tray line, appears on History as a triangle, and Apply is held back until it ends.
- [ ] Export the hourly summary as CSV with the app in Arabic and open it in Excel by double-clicking: the headers are in Arabic, the columns are split, the times are local, and there are no PCI or channel columns unless the box was ticked.
- [ ] Lower the retention (for example from 90 to 30 days) on a history older than that: the older records are gone within a minute of saving, and the disk use on the Settings card goes down.
- [ ] After a real revert, "See what happened" on the Bands-page banner opens History's event log on that change, with the reason.

## IP refresh (built 2026-11-07, not yet tried on the router)

When one of these is needed, the user gets the steps in chat.

- [ ] Read-only first: while connected, export diagnostics from the About page. In the file, the `/api/dialup/mobile-dataswitch` entry either shows `<dataswitch>1</dataswitch>` or an error code. Note which, and whether `/api/device/information` and `/api/monitoring/status` have a (redacted) `WanIPAddress` and `WanIPv6Address`. Nothing in this step changes the connection.
- [ ] One single Refresh IP from the Network page. The banner shows "Data off", the app stays connected, the router's web page still opens, and `events.jsonl` has no `session.lost`. Note the seconds until `ip_refresh.ended`.
- [ ] The switch reads on again afterwards, and the internet is back within 90 s.
- [ ] The Network page's range changes, or plausibly stays the same, within 10 s of "Online".
- [ ] Allowed bands, the LTE band and the 5G band are the same before and after; `net-mode` doesn't change in `events.jsonl`.
- [ ] Three single refreshes about a minute apart: note the ranges, and compare `ip_refresh.try` from/to with `off_s`.
- [ ] Quit from the tray while data is off: the app asks first. Start it again: data comes back on right after login.

## Refresh IP by network search, band switch and page scripts (built 2026-11-07, not yet tried on the router)

When one of these is needed, the user gets the steps in chat. The first one is read-only.

- [ ] **Page scripts first (read-only).** While connected, About › Router page scripts › Read page scripts, then Save zip. Note the file count, the size, and how many were missing or left out. `events.jsonl` has `diagnostics.page_scripts_saved` and no `session.lost`, and nothing on the router changed. If the summary lists `../js/…` references as left out, the path rule needs a second look. The zip is opened only to find the factory reset (`Control`) and APN calls.
- [ ] **One Refresh IP by network search.** The dialog says it uses a network search; the banner shows "Searching for networks" with the elapsed time, then the wait for the router to reconnect. Note the seconds the search took (`ip_refresh.search` `seconds`, `networks`, `timed_out`, `service_dropped`), whether `session.lost` appears, and the range before and after in `ip_refresh.ended`.
- [ ] The band lock, the LTE band and the 5G band are the same before and after (no `ip_refresh.bands_changed`). If they changed, "Restore my bands" puts them back through safe apply.
- [ ] Internet is back within 150 s of the search answering. If it isn't, the banner says the router hasn't reconnected yet and offers Restart router, which opens the two-step confirm (don't confirm unless needed).
- [ ] **Band switch for a new IP**, only after a search kept the same range: press "Try a band switch" and confirm. The safe-apply banner shows step 1 (the temporary bands) and step 2 (the bands from before). Note `ip_reattach.ended` (`outcome`, `from`, `to`) and that the final bands are the ones from before.
- [ ] Error codes: any `feature.refused`, `session.lost` or `ip_refresh.ended` refusal in `events.jsonl` now has a `code`. Note them.

## Log out (built 2026-11-07, not yet tried on the router)

- [ ] Start `ping -t 1.1.1.1`, log in to the router in a browser, then press Log out in Settings. No ping drops, the browser still works, and `events.jsonl` has `session.logout` with `ok: true` and router state `loggedOut`.
- [ ] Log out, then Connect: a full login, and no "Is this your router?" question.
- [ ] Log out, quit from the tray, start the app: it shows "Logged out" and makes no login attempt until Connect.

## APN profiles (built 2026-11-07, not yet tried on the router)

Every call here is from huawei-lte-api and unverified on the H122 (API_NOTES.md §8).

- [ ] A (read only): the Network page's list matches the router's own APN page, including "In use" and the lock icons. Events hold indexes only.
- [ ] B: add a profile "Test" that copies the one in use. It appears in the list and "In use" doesn't move. If it's refused with "may need an encrypted request", stop here and note it.
- [ ] C: switch to "Test" and back. Each switch reconnects and is kept after the trial; note any error code.
- [ ] D: edit "Test" to the APN `invalid.test` and switch to it. It must undo itself within about 90 s; note whether the status was 902, 904 or 906.
- [ ] Edit a profile that isn't in use and has a password, leaving the password blank: the app either keeps it or says the router hides it. Nothing is logged either way.
- [ ] E: delete "Test". It's gone from the list and from the router's page.

## History page (built 2026-11-07, not yet tried on the router)

- [ ] Reboot the router from its own web page, wait 10 minutes, and check the Signal card on Last 24 hours: "Lost service" and "Router restarted" count it, and the app's own band, APN and IP refresh minutes don't.
- [ ] The answer card, the More details sections (closed at first, remembered once opened) and Speed by IP range look right with real data.
- [ ] Release build: the Network page, the History sections and the IP refresh and APN banners work under the strict CSP, in English and in Arabic.

## Privacy check (built 2026-11-13; the first run on the router was the same day)

Read-only: the router gets GETs only and nothing on it changes. When it's needed, the user gets the steps in chat.

- [x] 2026-11-13 **Run check** on the Router page while connected: both parts gave a result, the app stayed logged in, and `events.jsonl` has `privacy.checked` (twice) and no `session.lost`.
- [x] 2026-11-13 From that line: `remoteManagement` ok and `devices` ok from `lan`. The element names are now in API_NOTES.md §9, and `enable` is the switch.
- [x] 2026-11-13 The line holds no server name, MAC address, device name, IP address or device count.
- [ ] **Remote management** names a server that looks like the operator's, not the router's own address, and says whether it is switched on.
- [ ] The devices part says which list the router gave (every remembered device, or only the ones on Wi-Fi now). **Show devices** lists the devices you expect, with the right online state; names in Arabic or with odd characters stay on their own line. **Hide devices** takes them off the screen. The Connection column says Wi-Fi or Cable correctly, the pictures fit the names, and this PC is marked.
- [ ] Leave the Router page and come back: the result is still there. Log out of the router: it is gone.
- [ ] The check is refused in words during a band change, a speed test and an auto-scan.
- [ ] **Check DNS** shows an address as two numbers and `x.x`, and **Show full** shows all of it. After turning on encrypted DNS in Windows and waiting a few minutes, the address changes.

## Factory reset button (changed 2026-11-13, not yet tried on the router)

- [ ] **Factory reset…** beside Restart opens the short warning. **Cancel** closes it, and nothing reaches the router: no `device.factory_reset` in `events.jsonl`. Don't confirm unless a reset is wanted.
- [ ] The button is off, with its reason beside it, during a band change, a speed test and while logged out.

## Update check, MVNO detection and the unsupported-router flow: UNTESTED ON DEVICE

Tested off-device only. Confirm:

- [ ] Update check, with the repo public and a release published: Allow, then Check for updates shows the notice, and Open release page opens the right page. While the repo is private, it can only ever say "No newer version found".
- [ ] MVNO: export diagnostics with an MVNO SIM, and look at the file's `sim` section. Does `homePlmn` match the code in `ui/src/lib/operators.ts`, what does `registeredPlmn` show, and is `spn` filled?
- [ ] Unsupported router: on a router that isn't an H122, with no password saved, the notice appears within about 15 s and names what it could find. The report reads correctly, the file saves, and the issue form opens filled in.
- [ ] On the H122 logged out, no notice appears.
