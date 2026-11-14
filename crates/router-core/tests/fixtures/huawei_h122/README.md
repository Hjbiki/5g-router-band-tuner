# Huawei H122-373 fixtures (firmware 11.0.2.21, Zain KSA)

Every file here is either a redacted real capture or marked synthetic. No raw capture is committed.

| File | Source | Redaction |
|---|---|---|
| `signal.xml` | Real `/api/device/signal` response | Location-identifying values replaced: `cell_id`, `enodeb_id`, `tac`, `pci`, `scc_pci`, `nei_cellid`. Signal levels, bands and frequencies are as captured. |
| `net_mode_list.xml` | Real `/api/net/net-mode-list` response | None needed |
| `net_mode.xml`, `monitoring_status.xml`, `current_plmn.xml`, `basic_information.xml`, `device_information.xml`, `state_login_*.xml`, `webserver_token.xml`, `challenge_login_*.xml`, `authentication_login_*.xml`, `token_headers.json`, `heartbeat.xml`, `host_info_ok.xml`, `error_lan_wan_config.xml` | Browser HAR capture, via `tools/fixtures/redact-har.mjs` | Tokens, nonces, salt, proofs, signatures, RSA key, serial, IMEI/IMSI/ICCID, MACs and IP addresses replaced with synthetic values of the same shape (IPs in 192.0.2.0/24 and 2001:db8::/32, MACs 02:00:00:00:00:xx) |
| `signal_reattach.xml`, `signal_bounds.xml` | Synthetic: derived from `signal.xml` by hand to cover edge cases | n/a |
| `plmn_list.xml` | Synthetic, in the shape huawei-lte-api documents for `/api/net/plmn-list`; the app only counts its `<Network>` entries | n/a: made-up operator names and test-network codes (MCC 001) |
| `dialup_profiles.xml` | Synthetic, in the shape huawei-lte-api documents for `/api/dialup/profiles`; not seen on this router yet | n/a: made-up names, APNs, username and password |

Facts the captures established:
- The `cell_id` format is `"<eNB, 7 digits>-<sector, 3 digits>"`, with a separate `enodeb_id`.
- 5G is NSA on **n41**: NR-ARFCN 528980, 2644.9 MHz, TDD.
- `net-mode-list` lists the supported LTE mask *before* an `"All bands"` entry, and has no `NrOptionList`.
