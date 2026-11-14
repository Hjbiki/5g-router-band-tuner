# Changelog

All notable changes to 5G Router Band Tuner. Versions follow [Semantic Versioning](https://semver.org/).

> **Going back to an older version:** a version without APN switching (0.1.0 and earlier) ignores an open APN change in `pending-apn-change.json`, and one without Refresh IP ignores mobile data left off in `pending-data-switch.json`. Let a change finish, or undo it, and make sure mobile data is on before you install an older version.

## [1.1.0] - 2026-11-14

### Added

- A portable version: one file that you just run, with no install. It is on every release and linked from the README.
- The first screen now starts with the language choice, with a flag on each language.
- The installer is now in Arabic as well as English, and asks which language you want when it starts.

## [1.0.0] - 2026-11-14

First public release. A Windows app that shows your Huawei router's 4G and 5G signal live and helps you find better bands. If a band change loses the internet, your previous bands come back by themselves. Tested on the Huawei H122-373 (firmware 11.0.2.21).

### What it does

- **Live 4G/5G signal:** strength, quality and interference in plain words, with a help tip on every number.
- **Safe band changes with auto-revert:** each change is saved, applied, checked and put on a trial. It's undone by itself if the internet is lost, even with the window closed or after a restart.
- **Try before you keep:** during a change's trial you can run a speed test, then keep the new bands or go back.
- **Primary band ⭐:** star a band to settle on it first, then allow the rest of your set. An optional watchdog locks it back when the router drifts.
- **Auto-scan:** tries band sets one at a time, measures each and ranks the results.
- **Built-in speed test:** Cloudflare by default, or the optional Ookla CLI, with results saved per band.
- **History:** which bands have been fastest for you, with the signal, every speed test and what happened, kept on this PC.
- **Refresh IP:** asks the network for a new internet address.
- **Privacy check:** shows whether your operator's remote management (TR-069) is set up on the router, the devices on your network and the DNS server your PC uses. It only reads.
- **Band history and operator badge:** see how bands behaved before, and which operator you're on.
- **Multi-language:** English, Arabic, more coming.
- **Private and local:** no account and no telemetry. Your router password stays in Windows Credential Manager, and the check for new versions is off until you allow it.
- **Help add your router:** if your router isn't supported, the app says so and helps you send a small report, with no password needed.

### Before you install

- **Windows 64-bit, H122-373 only for now.** Other routers haven't been tested.
- **Windows may warn you.** The installers aren't code-signed, so SmartScreen can show "Windows protected your PC". To continue, choose **More info**, then **Run anyway**.
- **Each band change drops your internet briefly** while the router reconnects, usually for under a minute.
- **Factory reset erases the router's settings.** It is a red button on the Router page, and it is sent only after you confirm a warning.
- **Some newer parts have had little testing on a real router.** [The device checklist](https://github.com/Hjbiki/5g-router-band-tuner/blob/main/docs/DEVICE_CHECKLIST.md) shows what has been confirmed.

Free to use, share and change for noncommercial purposes, under the PolyForm Noncommercial License 1.0.0. Unofficial, and not affiliated with or endorsed by Huawei or any operator.

## [0.1.0] - 2026-10-30

First release. Tested on a Huawei H122-373 (firmware 11.0.2.21).

### Added

- Live 4G and 5G signal, about once a second: RSRP, SINR, RSRQ, serving cell, carrier aggregation, and the correct 5G band name (for example n41).
- LTE band and 5G option editing, with a preview of exactly what will be sent to the router.
- Safe apply: the current settings are saved first, the change is checked, and if service doesn't come back the previous bands are restored automatically. This also works with the window closed, after a crash, and after the PC sleeps.
- Tray icon that shows the state at a glance (green, amber, red) and keeps watching changes in the background.
- Router reboot with a two-step confirmation.
- Presets, including an Auto preset.
- Arabic and English, with a right-to-left layout and optional Arabic-Indic digits; light and dark mode.
- Demo mode for trying the app without a router.

### Security

- Fully local: no account, no cloud, no telemetry. The router password is kept in Windows Credential Manager.
- No listening network ports, and a strict content security policy for the app window.
