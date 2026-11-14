# Changelog

All notable changes to 5G Router Band Tuner. Versions follow [Semantic Versioning](https://semver.org/).

> **Going back to an older version:** a version without APN switching (0.1.0 and earlier) ignores an open APN change in `pending-apn-change.json`, and one without Refresh IP ignores mobile data left off in `pending-data-switch.json`. Let a change finish, or undo it, and make sure mobile data is on before you install an older version.

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
