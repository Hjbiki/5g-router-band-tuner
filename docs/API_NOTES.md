# Huawei Router API Notes (from `huawei-manager.js`)

Target: Huawei 5G CPE **H122-373**, firmware **11.0.2.21**.
Source analysed: `huawei-manager.js`, a ~173 KB single-line bookmarklet that calls itself "Huawei Manager 2023 for Browser". It uses jQuery, which it borrows from the router's own web UI.

Confidence labels used below:
- **[script]**: read directly from the code.
- **[inferred]**: my reading of what the code implies. Not checked against the device.
- **[background]**: general knowledge of Huawei HiLink/CPE web APIs. **Not in the script.** Verify on the H122-373 before relying on it.

Functions are referred to by name because the original file is one line. A beautified copy (about 4,400 lines) was used for the analysis.

---

## 1. Endpoint inventory

All requests are same-origin `$.ajax` calls from the router's web UI page. Responses are XML. The script reads them either with `$.parseXML` + `$xml.find(tag)` or with a crude string split (`extractXML(tag, text)`).

### Read endpoints (GET, no CSRF header)

| Path | Called by | Polled? | Fields read |
|---|---|---|---|
| `/api/device/signal` | `Signl()` | every 300 ms | see §4 |
| `/api/monitoring/status` | `getStatus()` | every 300 ms | `ConnectionStatus`, `CurrentNetworkTypeEx`, `SignalIcon`, `SignalIconNr`, `PrimaryDns`, `SecondaryDns` |
| `/api/device/information` | `getInfo()` | every 300 ms | `DeviceName`, `SoftwareVersion`, `WebUIVersion`, `WanIPAddress`, `SecondWanIPAddress` |
| `/api/net/net-mode` | `getNetmode()` | every 300 ms | `NetworkMode`, `NetworkBand`, `LTEBand`, `networkOption` |
| `/api/net/net-mode-list` | `ReadHardBand()` | every 300 ms | `Access`, `BandList/…/Value`, first `LTEBand/Value` |
| `/api/net/current-plmn` | `PlmnPh()` | every 300 ms | `FullName`, `Numeric` (MCC+MNC) |
| `/api/device/boot_time` | `DeviceBoot()` | every 300 ms | `boot_time` (format like `[d]:[h]:[m]:[s]`; the script strips the brackets) |
| `/api/device/antenna_type` | `getantennastatus()`, `ReadAntenna()` | every 300 ms (through `getInfo`) | `antenna1type`, `antenna2type` (`0` internal, `1` external) |
| `/api/device/antenna_status` | same | every 300 ms | `antenna1_status`, `antenna2_status` (`0` internal, `1` external) |
| `/api/device/antenna_set_type` | same | every 300 ms | `antennasettype` (`0` auto, `1` ext, `2` int, `3` mixed) |
| `/api/device/antenna_settings` | `ReadAntenna()` | on button click | `antenna_type` (`3` auto, `0` int, `1` ext, `2` mixed). **Encoding differs from `antenna_set_type`.** |
| `/api/net/plmn-list` | `changeip()` | on button click | nothing (the handler is broken, see §6) |

Example response shape [background]:
```xml
<?xml version="1.0" encoding="UTF-8"?>
<response><NetworkMode>00</NetworkMode><NetworkBand>3FFFFFFF</NetworkBand><LTEBand>7FFFFFFFFFFFFFFF</LTEBand><networkOption>0</networkOption></response>
```

### Write endpoints (POST, CSRF header required)

Every write uses the same pattern (see §2): `Content-Type: application/xml`, header `__RequestVerificationToken: <token>`, and a body wrapped in `<request>…</request>`.

| Path | Called by | Body |
|---|---|---|
| `/api/net/net-mode` | `refresh()` | `<request><NetworkMode>{m}</NetworkMode><NetworkBand>{nb}</NetworkBand><LTEBand>{lb}</LTEBand><networkOption>{opt}</networkOption></request>` |
| `/api/net/net-mode` | `bandselect()` | `<request><NetworkMode>00</NetworkMode><NetworkBand>3FFFFFFF</NetworkBand><LTEBand>{lb}</LTEBand></request>` (no `networkOption`) |
| `/api/device/control` | `rest(1)`, `restor(2)`, `power(4)` | `<request><Control>{n}</Control></request>` |
| `/api/device/antenna_set_type` | `SaveAnte()` | `<request><antennasettype>{0..3}</antennasettype></request>` |
| `/api/device/antenna_settings` | `SaveAnteo()` | `<request><antenna_type>{0..3}</antenna_type></request>` |

`/api/device/control` values [script]: `1` = reboot, `2` = factory reset, `4` = power off. (`3` is commonly "backup config" on Huawei devices [background]; the script doesn't use it.)

Success handling [script]: any HTTP 2xx counts as success, so "Request Done" appears. The response body is never inspected. Huawei APIs usually return errors as **HTTP 200** with `<error><code>…</code><message/></error>` [background], so the script can report success when the router rejected the write. Common codes [background]: `125002`/`125003` CSRF token invalid, `100003` no permission / not logged in, `100005` bad parameter, `108003` session limit.

---

## 2. Authentication and session flow

### What the script does [script]
- **No login.** It assumes you're already logged into the router's web UI in the same tab, and it rides the existing session cookie. jQuery same-origin requests send cookies automatically.
- **No explicit cookie handling.** It never reads or sets cookies.
- **CSRF token acquisition before each write:**
  1. `$.ajax({type: "GET", dataType: "text"})` with **no `url`**. jQuery then GETs the *current page URL*, i.e. re-downloads the web UI HTML.
  2. Splits the HTML on `name="csrf_token" content="` and takes the **last** match up to the next `"`.
  3. Waits **2 seconds** (`setTimeout(…, 2e3)`).
  4. POSTs with header `__RequestVerificationToken: <token>`.
- It never reads the `__RequestVerificationToken` response header, so it doesn't follow token rotation. It sidesteps rotation by scraping a fresh page before every write.
- GETs send no token.

### What a standalone client would need [background, verify on device]
The script needs a logged-in browser tab, so a standalone tool has to reproduce the login itself. On Huawei CPE / HiLink firmware the flow is usually one of these:

1. **Get a session plus an initial token.** Either `GET /api/webserver/SesTokInfo` → `<SesInfo>SessionID=…</SesInfo><TokInfo>…</TokInfo>`, or `GET /` and scrape the `<meta name="csrf_token">` tags. Newer UIs emit two of them, which is presumably why the script takes the last one.
2. **Log in.** One of:
   - **Legacy:** `POST /api/user/login` with `<Username>`, `<Password>`, `<password_type>4</password_type>`, where Password = `base64(sha256hex(username + base64(sha256hex(password)) + token)))`.
   - **SCRAM (newer CPE firmware, likely on 11.x):** `POST /api/user/challenge_login` (username, client nonce, `<type>`) → server nonce, salt, iterations; then `POST /api/user/authentication_login` (client proof). This uses PBKDF2-SHA256 and HMAC.
3. After login the server sets a new `SessionID` cookie (often `HttpOnly`) and returns new tokens in response headers (`__RequestVerificationToken`, sometimes `__RequestVerificationTokenone`/`…two`).
4. **Tokens are often single-use.** Each POST response carries the next token in its headers. Reusing a token gives `125002`/`125003`.
5. Sessions time out after a few minutes idle. The script's 300 ms polling keeps the session alive indefinitely.

**Open questions to check on the H122-373 (DevTools → Network tab, during a manual login):** which login endpoint it uses, the exact cookie name, whether `SesTokInfo` exists, and whether tokens are single-use.

---

## 3. Band locking encoding

All band and mode control goes through one endpoint: `POST /api/net/net-mode`.

### `NetworkMode` (RAT selection) [script]
| Value | Meaning in script |
|---|---|
| `00` | Auto (all RATs incl. 5G) |
| `01` | 2G only |
| `02` | 3G only |
| `03` | 4G only |

Other values Huawei 5G firmware may accept, such as `08` (NR only) or combined strings like `0803` [background], are **not used** by the script. Check `GET /api/net/net-mode-list` → `<AccessList>` to see what the H122-373 offers.

### `NetworkBand` (2G/3G bitmask, hex string) [script]
| Value | Band |
|---|---|
| `80` | GSM 1800 |
| `300` | GSM 900 |
| `80000` | GSM 850 |
| `400000` | WCDMA B1 (2100) |
| `800000` | WCDMA B2 (1900) |
| `2000000` | WCDMA B4 (1700) |
| `4000000` | WCDMA B5 (850) |
| `8000000` | WCDMA B6 (800) |
| `2000000000000` | WCDMA B8 (900) |
| `4000000000000` | WCDMA B9 (1700) |
| `1000000000000000` | WCDMA B19 (800) |
| `3FFFFFFF` | "Auto" (all 2G/3G) |
| `00000000` | no 2G/3G (used for the 5G modes) |

The 3G radio buttons are mutually exclusive, so you can lock exactly one WCDMA band.

### `LTEBand` (LTE bitmask, hex string) [script]
- **Bit (N-1) = LTE band N.** B1 = `0x1`, B3 = `0x4`, B7 = `0x40`, B20 = `0x80000`, B28 = `0x8000000`, B40 = `0x8000000000`, B42 = `0x20000000000`.
- `7FFFFFFFFFFFFFFF` = all bands (Auto).
- Hex without a `0x` prefix. Case doesn't seem to matter; the script mixes upper and lower case.
- Example: B1+B3+B20 = `0x80005` → send `80005`.

The script also treats a hard-coded whitelist of masks as meaning "Auto" for display only (`_4GType`, `B_4GType`): `20800800C5`, `7FFFFFFFFFFFFFFF`, `1A000000001`, `1A000000085`, `2000000085`, `7E2880800D5`, `7A0880800D5`, `20000800C5`, `20880800C5`, `3E2880800D5`, `1A08A0C00DD`, `7E2880800D0`, `1A08A0C00DF`, `A0880800C1`, `A0880800C5`, `1A008080085`, `7A0000800C5`, `1A000080085`, `1E20A000095`, `7E2880E00D5`, `2000000680380`. These look like per-model or per-operator "all supported bands" defaults [inferred]. If the H122-373's default isn't in the list, the UI shows the individual bands instead of "Auto".

**Supported bands** come from `GET /api/net/net-mode-list` [script]: the first `<LTEBand><Value>` is treated as the hardware-supported LTE mask, and the `<BandList>` `<Value>`s as the 2G/3G masks. `<Access>` containing `01`/`02` decides whether the 2G/3G buttons appear.

### `networkOption` (5G NSA/SA) [script]
| Value | Meaning |
|---|---|
| `0` | 5G NSA (anchored on LTE) |
| `1` | 5G SA |
| `2` | SA + NSA |

The field is only present on 5G models. The script reads it from `GET /api/net/net-mode` and hides the field if it's absent.

### What each UI action sends [script]
| Action | NetworkMode | NetworkBand | LTEBand | networkOption |
|---|---|---|---|---|
| AUTO button | `00` | `3FFFFFFF` | `7FFF…F` | **`undefined`** (bug) |
| 5G button | `00` | `00000000` | `7FFF…F` | **`undefined`** (bug) |
| 4G / 3G / 2G buttons | `03` / `02` / `01` | `3FFFFFFF` | `7FFF…F` | **`undefined`** (bug) |
| 3G single band | `02` | WCDMA mask | `7FFF…F` | **`undefined`** (bug) |
| LTE bands, "4G+5G" unchecked | `03` | `3FFFFFFF` | mask | **`undefined`** (bug) |
| LTE bands, "4G+5G" checked | `00` | `00000000` | mask | selected radio |
| Only an NSA/SA radio | `00` | `00000000` | `7FFF…F` | `0`/`1`/`2` |
| "Combine BAND" prompt (`bandselect`) | `00` | `3FFFFFFF` | mask | *(omitted)* |

**There's no 5G NR band locking anywhere in the script.** It only selects LTE bands and the SA/NSA mode. NR band locking on this model would need a different endpoint or field, which is unknown. Look for one in the stock UI's JS.

`bandselect` "main band" trick [script]: entering e.g. `3m+1+20` first locks only B3, waits for success plus 2 s, then locks B3+B1+B20. The idea is to make the modem camp on B3 as the primary cell before widening the mask. Without an `m` it throws a harmless `TypeError` (`_2ndrun.join` on `null`) after succeeding.

---

## 4. Signal fields (`GET /api/device/signal`)

Values come back as strings with units and sometimes comparison prefixes like `>=-44dBm` or `&lt;-140dBm`. The script strips `<`, `>`, `=`, `dBm`, `dB`, and replaces `Unknown` with `0`.

### Cell identity
| Field | Meaning |
|---|---|
| `mode` | Serving RAT: `0` = 2G, `2` = 3G, `7` = 4G (LTE / 5G NSA anchor). Other values (e.g. SA) aren't handled [script]. |
| `band` | Serving LTE band number (shown as `B<n>`) |
| `cell_id` | Cell ID (decimal). For LTE, hex digits minus the last 2 = **eNB ID**, last 2 = sector/cell (ECI = eNB×256 + CID). For 3G, the split is at the last 4 hex digits (RNC/CID). NR 36-bit NCI isn't decoded. |
| `pci` / `sc` | Physical Cell ID (LTE/NR) / scrambling code (3G) |
| `tac` / `rac` / `lac` | Tracking area (4G) / routing area (3G) / location area (2G) |
| `earfcn` | LTE channel, format `DL:<n> UL:<n>` |
| `wdlfreq` | 3G DL channel (UARFCN) |
| `arfcn` | 2G ARFCN |
| `nrearfcn` | NR channel, format `DL:<n> UL:<n>`. The script maps DL NR-ARFCN → `n<band>` via `arfcntoband()`. |

### LTE radio quality
| Field | Meaning | Script's colour scale (bad → excellent) |
|---|---|---|
| `rsrp` | Reference Signal Received Power, dBm. Main "signal strength" metric. | -124 … -51 |
| `rsrq` | Reference Signal Received Quality, dB. Load/interference. | -30 … 0 |
| `sinr` | Signal-to-Interference-plus-Noise Ratio, dB. Best predictor of throughput. | -15 … 30 |
| `rssi` | Total received wideband power, dBm (includes noise and neighbours) | -124 … -51 |
| `txpower` | UE transmit power, format `PPusch:XdBm PPucch:XdBm PSrs:XdBm PPrach:XdBm` (PUSCH data, PUCCH control, SRS sounding, PRACH random access). High values (~23 dBm max) = far from tower. | -20 … 23 (uses PPucch) |
| `dlbandwidth` / `ulbandwidth` | Channel bandwidth (`1400KHz`, `5MHz` … `20MHz`) | |
| `dlfrequency` / `ulfrequency` | Carrier frequency in kHz | |
| `ltedlfreq` / `lteulfreq` | LTE frequency (alternative fields) | |
| `dl_mcs` | DL Modulation & Coding Scheme per codeword, format `mcsDownCarrier1Code0:X mcsDownCarrier1Code1:Y` (parsed by fixed offset 21). Higher = better modulation. | 4 … 32 |
| `ul_mcs` | UL MCS, format `mcsUpCarrier1:X` | 4 … 28 |
| `cqi0` / `cqi1` | Channel Quality Indicator per codeword (0–15). `32639` = invalid/absent. | 2 … 18 |

The script derives QAM order and TBS index from MCS with a hard-coded table. These are display estimates, not values the router reports.

### 3G
| Field | Meaning |
|---|---|
| `rscp` | Received Signal Code Power, dBm (-115 … -60) |
| `ecio` | Ec/Io, dB, pilot quality (-20 … 0) |
| `txpower` | 3G form: `stxpwr:XdBm` |

### 5G NR
| Field | Meaning | Scale |
|---|---|---|
| `nrrsrp` | NR SS-RSRP, dBm | -124 … -85 |
| `nrrsrq` | NR SS-RSRQ, dB | -20 … 0 |
| `nrsinr` | NR SS-SINR, dB | -10 … 30 |
| `nrtxpower` | NR UE tx power, same `PPusch:…` format as LTE | -20 … 23 |
| `nrdlbandwidth` / `nrulbandwidth` | NR channel BW (e.g. `100MHz`) | |
| `nrdlfreq` / `nrulfreq` | NR carrier freq (kHz) | |
| `nrdlmcs` | `NRmcsDownCarrier1Code0:X NRmcsDownCarrier1Code1:Y` (fixed offset 23) | |
| `nrulmcs` | `NRmcsUpCarrier1:X` | |

The presence of `<nrrsrp>` is how the script decides whether to show the 5G panel at all.

### `/api/monitoring/status` codes [script]
- `ConnectionStatus`: `900` connecting, `901` connected, `902` disconnected, `903` disconnecting, `904` failed, `905` no service, `906` error.
- `CurrentNetworkTypeEx`: `0` none, `1` GSM, `2` GPRS, `3` EDGE, `41` UMTS, `42` HSDPA, `43` HSUPA, `44` HSPA, `45` HSPA+, `46` DC-HSPA+, `101` LTE, `1011` LTE CA, `111` NR.
- `SignalIcon` / `SignalIconNr`: 0–5 bars for LTE / NR.

---

## 5. Model- and firmware-specific things

- **Two antenna APIs with conflicting encodings.** `antenna_set_type`/`antennasettype` uses 0 auto, 1 ext, 2 int, 3 mixed. `antenna_settings`/`antenna_type` uses 3 auto, 0 int, 1 ext, 2 mixed. The script tries both and shows whichever returns data. Which one the H122-373 implements is unknown; it may implement neither (the panel then hides itself).
- **"Auto" mask whitelist** (§3) is a list of device-specific default masks.
- **Stock-UI DOM IDs.** Every 300 ms, `Main()` force-shows ~45 element IDs from the Huawei web UI that the firmware normally hides, such as `developer_mode_div`, `mlog`, `calltrace`, `apklog`, `antenna_page`, `LTE_band_control_father`, `mobilesearch_LTE_band_select`, `eths-manual-mac`. These IDs are tied to a specific web-UI generation, and they show the stock UI has its own hidden band-lock and antenna controls.
- **Fixed string offsets** for parsing `dl_mcs` (21) and `nrdlmcs` (23) assume exact field-name formats. Different firmware wording would break them silently.
- **`networkOption`** exists only on 5G firmware.
- **`mode` values**: only 0/2/7 are handled. A 5G SA connection may report a value the script ignores [inferred].
- **CSRF via `<meta name="csrf_token">`** in the page HTML (taking the last of possibly several) is the newer Huawei web-UI style, not the older `SesTokInfo` style [inferred].
- **LTE B-number → MHz labels** and the NR-ARFCN → band table are generic 3GPP data, not model-specific.

---

## 6. Risks and bugs

### Can disconnect or brick-until-reset
1. **Factory reset with one click.** "Restore Default Settings" → `Control=2`, with **no confirmation**. It wipes APN, Wi-Fi, admin password and every other setting.
2. **Power off with one click.** "ShutDown Router" → `Control=4`, no confirmation. Recovery needs physical access (unplug and replug).
3. **Reboot**: `Control=1`, no confirmation. A short outage.
4. **Band lock to a band your operator doesn't serve at your location** → no service. You can recover over LAN by setting Auto, but anything remote loses its connection.
5. **SA-only (`networkOption=1`)** where SA isn't deployed → no service.
6. **"4G only" (`NetworkMode=03`)** turns off 5G entirely. Any LTE lock without the "Enabled 4G + 5G" box also drops 5G.
7. **Antenna set to External with no external antenna attached** → large signal loss or disconnect.
8. **`changeip()` / "Refresh IP Address"**:
   - `GET /api/net/plmn-list` triggers a manual operator scan on Huawei devices, which typically drops the data session for tens of seconds [background]. That's presumably the "new IP" trick.
   - The success handler references an undefined `xml`, so it throws a `ReferenceError`.
   - If `xml` were defined, the `while` loop never shrinks its input, so it would spin forever and freeze the tab.
   - **The app now uses this GET as the H122's Refresh IP** (since 2026-11-07, untested on the router), with its own safeguards instead of the script's: one search at a time from a task of its own with a 180 s timeout, the session refreshed first, the router's answer reduced to a count of networks (names and codes are dropped and never logged), the hourly budget and prefix loop of Refresh IP, an online check afterwards (in service plus a WAN address within 150 s), the band lock read again afterwards, and no restart unless the user confirms one. It never POSTs `net-register`, so it shouldn't change the operator or the band lock. See §7 "Network search".

### Encoding bugs that send the wrong configuration
9. **`networkOption=undefined` is sent literally.** `refresh(val, val2, val3, nroption)` shadows the global `nroption`, and most callers pass only 3 arguments. So AUTO, 2G/3G/4G/5G, 3G locks and plain 4G locks all POST `<networkOption>undefined</networkOption>`. The firmware may reject it with HTTP 200 + `<error>` (which the script reports as success), ignore it, or store something odd. **This is unverified and the most important thing to check.**
10. **Wrong LTE masks from decimal arithmetic.** `setBands()` builds the mask by adding hex-looking constants as *decimal* numbers (B5 adds `10`, B8 adds `80`, B20 adds `80000` …) and then sends the decimal string as hex. That only works while no hex nibble sums past 9. Examples:
    - B2+B4 → `10` → B5
    - B3+B4 → `12` → B2+B5
    - B1+B2+B3+B4 → `15` → B1+B3+B5
    - B38+B40 → `10000000000` → B41

    It also drops B27: the checkbox exists but `setBands` never reads it. The `lbm` variable *is* computed correctly but never used.
11. **`setBands` depends on globals `b1…b53`**, which are only defined as a side effect of `B_4GType()` being called by the poller.
12. **NR band naming is wrong for common bands** (`arfcntoband`: first matching range wins):
    - Every n78 channel (620000–653333) is reported as **n77**.
    - n41 channels ≥ 524000 are reported as **n7**.
    - n66 is never returned (n65 matches first).
    - Exact boundary values return `undefined` (strict `<`/`>`).

    Display only, but misleading.
13. **Bandwidth checks ignore UL.** `$xml.find('dlbandwidth','ulbandwidth')` ignores its second argument, so the colour and percentage reflect DL bandwidth only.
14. **Precision.** `B_4GType` uses `parseInt` + float `BitwiseAndLarge` on 64-bit masks. That's fine for bands ≤ 53, but masks with high bits set lose low-bit precision. `_4GType` uses `BigInt` correctly.

### Operational
15. **Request flood.** `Main()` runs every **300 ms** and fires ~11 async GETs each tick (8 directly + 3 antenna calls via `getInfo`). That's roughly **35 requests/second**, with no back-off and no "previous request still pending" check. On a slow router, requests pile up. It may also trip session or rate limits, and it keeps the session alive forever.
16. **Success is reported for any HTTP 200**, including Huawei `<error>` responses (see §1).
17. **The CSRF scrape fails silently** if the bookmarklet runs on a page without a `csrf_token` meta tag (e.g. the login page). The "token" becomes the first chunk of HTML before a `"`.
18. **Hidden settings get unhidden.** Developer-mode, logging and call-trace controls in the stock UI become visible and clickable. The firmware hides them for a reason.
19. **Sensitive data.** `/api/device/information` normally also returns IMEI, IMSI, serial number, MAC and phone number [background]. The script shows only model, versions and WAN IP, but any tool that logs the raw response will capture them.

---

## 7. Observed on the H122-373 (firmware 11.0.2.21)

Taken from a browser capture of the stock web UI. No secrets or identifying values are recorded here. Items under "To confirm" are checked by `docs/DEVICE_CHECKLIST.md`.

### Login (SCRAM)
- `GET /` (the web page) sets an `HttpOnly` `SessionID` cookie. The API GETs (`state-login`, `webserver/token`) set none. The **first login of a fresh process** is refused on `challenge_login` with **125003**, while a later attempt in the same process succeeds; this was seen twice on 2026-10-29. The app therefore loads `/` before logging in, and on a 125001–125003 at the challenge it reloads `/`, fetches a new token and resends the challenge once. The token is checked before any credential, and no proof has been sent at that point. `authentication_login` (the proof) is never resent.
1. `GET /api/user/state-login` reports the login state.
2. `GET /api/webserver/token` returns a token; use its last 32 characters.
3. `POST /api/user/challenge_login` with `{username, firstnonce, mode=1}` returns `{salt, servernonce, iterations}`. `iterations` is 1000.
4. `POST /api/user/authentication_login` with `{clientproof, finalnonce}` returns `serversignature` plus a pool of CSRF tokens.
5. The client verifies the server signature. HMAC argument order follows the router's own `SCRAMJS`.

### CSRF tokens
- Tokens are single-use.
- Harvest them from the `...one` / `...two` response headers if present; otherwise split the `#`-joined pool.
- A heartbeat request every 5 s keeps the session alive.

### Required request headers
- `X-Requested-With`.
- `_ResponseSource: Broswer` (sic).
- POSTs use the XML content type the web UI uses.

### `net-mode` and `net-mode-list`
- `net-mode` has the shape documented in section 3 (`NetworkMode`, `NetworkBand`, `LTEBand`, `networkOption`).
- `net-mode-list` lists the supported LTE mask before an "All bands" entry. It has no `NrOptionList`.
- `AccessList` values seen: `00`, `08`, `03`.

### Signal
- `cell_id` is `"<eNB, 7 digits>-<sector, 3 digits>"`, with a separate `enodeb_id`.
- NR-ARFCN 528980 is 2644.9 MHz, TDD, band **n41**. The bookmarklet labels it n7, which is wrong.
- `scc_pci` is present when carrier aggregation is active.
- `nrcqi0` / `nrcqi1` use 32639 for "invalid".

### Sessions (device test 2026-10-29)
- **Concurrent logins are allowed:** a browser login and the app stayed logged in at the same time, with no error on either side; 108003 never appeared. A web-UI login is not a takeover on this firmware.
- **The session expires 300 s after the last connect step,** even while data GETs and `/api/user/heartbeat` run every 1-5 s. A connect that only reused the session (state-login said logged in) restarted the clock, so one of state-login, device/information or net-mode-list refreshes it. After expiry, data GETs answer **125002**, not 100003. The app treats token errors on data requests as a session loss, and repeats the connect reads every 270 s; the session then held for the rest of the test.
- **During a re-attach the router answers slowly:** request timeouts, an incomplete `monitoring/status` (no `ConnectionStatus`), and for a few seconds after a POST the old `net-mode`.

### Factory reset: confirmed on 2026-11-08 (first checked 2026-10-31)

**Update 2026-11-08:** confirmed on a real H122-373 (fw 11.0.2.21): the bookmarklet resets it with `POST /api/device/control` `<Control>2</Control>`, and that it also reboots, changes bands and refreshes the IP on it. Reboot on the same endpoint (`Control=1`) is device-tested. So the driver now sets `factory_reset_verified: true` and sends exactly that call. The app's own reset run is still to be seen on hardware. What follows is the earlier check.

The `2` = factory reset value earlier in these notes comes from a third-party script, not from this firmware. To confirm it, the router's own web UI was read with GETs only, without logging in; nothing was POSTed.

- Public scripts (`lib/emui-jquery.js`, `js/public.js`, `js/guide.js`, `js/index.js`, `js/redirect.js`): `device/control` appears only with `Control: 1` (reboot).
- `language/lang_en_us.js` has a Reset menu entry and "Restoring factory settings…" text, so the feature exists, but text doesn't show the call.
- `html/content.html` redirects to the login page without a session, and any script that isn't public answers `100003`, so the Reset page's code can't be read this way.

So `factory_reset_verified` stays false for this driver, and `RouterDriver::factory_reset` keeps its default, which sends nothing. To enable it, read the Reset page's code in a logged-in browser session and record the exact request here.

### Factory reset: what other projects use (checked 2026-10-31)

This is what third-party code sends. It isn't what any firmware is confirmed to accept, and it doesn't change the status above: nothing here has been sent to a router.

- **[huawei-lte-api](https://github.com/Salamek/huawei-lte-api)** names the H122-373 among its supported models, and has two candidates:
  - `POST /api/device/control` with `<Control>2</Control>`. `huawei_lte_api/enums/device.py` has `ControlModeEnum`: `REBOOT = 1`, `RESET = 2` ("Resets device into factory settings"), `BACKUP_CONFIGURATION = 3`, `POWER_OFF = 4`. `Device.set_control` posts it. This matches the value noted earlier in this file.
  - `POST /api/device/basic_information` with `restore_default_status` set to 1 (`Device.set_basic_information`). What it does on a given firmware isn't documented there, so treat it as unknown.
- **[huawei_manager_py](https://github.com/sapy22/huawei_manager_py)** (checked at `2d990a5`) is a desktop app built on huawei-lte-api. Its Reset button calls `client.device.set_control(control=ControlModeEnum.RESET)`, which is the same `Control=2`, after one confirmation. Its README says the app was tested on the H122-373 and H122-370, but nothing says the reset itself was tried. It doesn't use `restore_default_status`.

Both projects were only read: none of their code is used here. huawei-lte-api is LGPL-3.0 and huawei_manager_py is MIT.

### NR band lock and cell lock: what other projects have (checked 2026-11-01)

Neither project sets an NR band or locks a cell, so this gives no way to do either.

- **NR band lock:** huawei_manager_py only sets `net/net-mode` with `NetworkMode`, `NetworkBand` and `LTEBand`, as this app does. huawei-lte-api has no NR band field either.
- **Cell lock:** huawei-lte-api has `GET /api/ntwk/celllock` ("Get the cell lock status"), with no call that sets it. Whether the H122 answers it is unknown. huawei_manager_py doesn't use it.

These are worth including in the M4 spike. All are reads except the last, and none has been tried here:

- `GET /api/ntwk/celllock`: does it exist on this firmware, and which fields does it name?
- `GET /api/net/net-feature-switch` and `GET /api/net/cell-info`: these may say what the firmware allows, and which cells it sees.
- `EndcStatus` and `endcRestrictedStatus` in `monitoring/status`: huawei_manager_py's notes read these as "NR dual connectivity on" and "restricted from it". The parser doesn't read them yet. If that holds on the device, they could help explain why a band set has no 5G.
- `POST /api/net/reconnect` with `ReconnectAction=1`: a change to the connection, so it would need the same care as a band change.

huawei_manager_py's notes also give `nroption` as 0 = NSA, 1 = SA and 2 = SA + NSA, and call `7E2880800D5` "auto for 5G??". Both are its author's notes, not confirmed here.

**Testing happens on a spare H158-381, never on the H122.**

### Data switch (seen 2026-11-07)

The names come from huawei-lte-api (`huawei_lte_api/api/DialUp.py`), which was only read.

- `GET /api/dialup/mobile-dataswitch` returns `<response><dataswitch>1</dataswitch></response>`: 1 is mobile data on, 0 is off. huawei-lte-api reads it as `DialUp.mobile_dataswitch`. **On the H122 the GET works** (the capability read it on 2026-11-07).
- `POST /api/dialup/mobile-dataswitch` with `<request><dataswitch>0</dataswitch></request>` or `1` sets it (`DialUp.set_mobile_dataswitch`). **On the H122 the "off" was refused** on 2026-11-07 with a session-type error (the run ended `offRefused` and data never went off), while the session itself was fine. So the H122 driver doesn't offer this method (`Capabilities.ip_methods` is the network search only). Routers that accept it still use it, and a refused "off" now falls back to the network search in the same try instead of counting as a lost session.
- The diagnostics export reads the GET (`DIAGNOSTIC_PATHS` in `drivers/huawei/driver.rs`). The capabilities read calls it too: it decides the "mobile data is off" notice and the Turn mobile data on button.
- `net/reconnect` and `dialup/dial` are not used.

### Network search (unverified)

- `GET /api/net/plmn-list` (huawei-lte-api `Net.plmn_list`, and the bookmarklet's "Refresh IP Address") asks the router for a manual operator search. On Huawei routers this usually drops the data session and answers in 30 to 120 s, longer in NSA; the router then reconnects by itself, usually with a new address [background]. A band re-attach on 2026-11-07 changed the WAN range, which fits that.
- The answer lists networks (`<Network>` entries with names and codes). The driver keeps only how many there were; the body is dropped and never logged, and the path is not in `DIAGNOSTIC_PATHS` (a unit test checks it), the probe doesn't read it, and the page-script saver refuses any `/api/` path.
- Unknown on this firmware: how long it takes, whether the app's session survives it (the app refreshes the session first and copes with a loss either way), whether the address really changes, and whether it leaves `net-mode` alone. The app reads the band lock again after every search and offers to put it back if it changed.
- A quick refusal (an answer in under 5 s with a router error) ends the run as "search refused", with the code, and the app stops offering the search until the next Connect.

### Web UI scripts

- Before login the router's own scripts (`public.js`) only show `device/control` for the reboot. The settings pages (APN, mobile network, factory reset) load only after login, from `/html/content.html`, which answers 307 to `/html/index.html` when logged out.
- Static `/js/*.js` files come as gzip bodies, sometimes without a `Content-Encoding` header; an unknown name answers an 88-byte error page.
- **About › Router page scripts** saves these files with the app's logged-in session, GETs only, so the factory reset call (and later the APN calls) can be read in the firmware's own code instead of being sent to find out. The rules are in `HOW_IT_WORKS.md` "Router page scripts". The zip is the maker's code: keep it private and out of the repo.

### Logout (unverified)

- `POST /api/user/logout` with `<request><Logout>1</Logout></request>` (huawei-lte-api `User.logout`). The app sends it once and waits at most 5 s. "No rights" (100003) and a token error that survives the retry both read as "already logged out". Whether it ends only the app's own session (a browser logged in at the same time should stay logged in) and leaves the internet up hasn't been seen yet.
- After it, the app drops its cookie jar and tokens, so the next Connect starts from `GET /` and a full SCRAM login (the 125003 resend applies).

### WAN address (unverified)

- `GET /api/device/information` holds `WanIPAddress` and `WanIPv6Address` (the fixture has both). The parser reads only these two for the address: an empty, `0.0.0.0` or unparsable IPv4 address means no data connection, and of IPv6 only whether there is one is kept. Whether the values are live, and how soon they change after a reconnect, is unknown.
- Whether `/api/monitoring/status` also carries `WanIPAddress` on this firmware is unknown; the app doesn't rely on it.
- Only the first two numbers (`a.b`) are ever written anywhere. See HOW_IT_WORKS.md "IP address privacy".

### To confirm on the device
- Which GET endpoints work without a session.
- Whether POSTs are accepted with lowercase header names (as sent by the Rust HTTP client).

---

## 8. APN profiles (dialup)

Nothing in this section has been seen on the H122. Every field and call is **[huawei-lte-api, unverified]**: the names come from huawei-lte-api's `DialUp` (`huawei_lte_api/api/DialUp.py`), which was only read. The bodies hold the saved APN passwords, so they are parsed and dropped in the driver, never logged, and not part of the diagnostics export.

### Reads [huawei-lte-api, unverified]

- `GET /api/dialup/profiles`: `<CurrentProfile>` (the index in use) and `<Profiles>` with one `<Profile>` each: `Index`, `IsValid` (1 = complete), `Name`, `ApnIsStatic` (1 = the profile sets the APN, 0 = the network picks it), `ApnName`, `DialupNum` (`*99#`), `Username`, `Password`, `AuthMode` (0 = auto, 1 = PAP, 2 = CHAP), `IpIsStatic`, `IpAddress`, `DnsIsStatic`, `PrimaryDns`, `SecondaryDns`, `ReadOnly` (anything but 0 is treated as the operator's), `iptype` (0 = IPv4, 1 = IPv6, 2 = IPv4 and IPv6).
- `Password` may come back in plain text, empty or as stars. That decides whether "keep the saved password" can work: the app refuses it when the text is empty or only `*`.
- `GET /api/dialup/auto-apn`: `<AutoAPN>` 1 when the router picks the profile by itself. While it's on, the app doesn't switch.
- The app keeps, per profile, everything but the password (only whether one is set), plus a fingerprint: 16 hex characters of SHA-256 over name, APN, username, `AuthMode` and `iptype`. Every write checks it before and after.

### Writes [huawei-lte-api, unverified]

All go to `POST /api/dialup/profiles`. huawei-lte-api sends the add and the edit encrypted (`is_encrypted=True`); the app sends plain XML, and a refusal other than "busy" is reported as "may need an encrypted request".

- **Switch** (`set_default_profile`): `<Delete>0</Delete><SetDefault>{index}</SetDefault><Modify>0</Modify>`. Through safe apply, see HOW_IT_WORKS.md. Whether it's refused while data is connected is unknown; any error code is recorded as a rejected apply.
- **Add** (`create_profile`): `<Delete>0</Delete><SetDefault>{current}</SetDefault><Modify>1</Modify><Profile>…</Profile>` with `Index` empty, `IsValid` 1, `ReadOnly` 0 and `DialupNum` `*99#`. huawei-lte-api sends `SetDefault` 0 or 1 here; the app names the profile in use instead, so the default can't move either way.
- **Edit** (`update_profile`): `Modify` 2, with the whole `<Profile>` from a fresh read: the app replaces `Name`, `ApnIsStatic`, `ApnName`, `Username`, `Password`, `AuthMode` and `iptype`, and sends every other tag back as the router had it.
- **Delete** (`delete_profile`): `<Delete>{index}</Delete><SetDefault>{current}</SetDefault><Modify>0</Modify>`.
- Before each write the driver reads the list again and sends nothing unless the profile in use is still the one from the app's last read, and the target is there, unchanged, complete, not in use and not the operator's. After it, it reads again: if the router now dials with another profile, the app switches back with automatic undo (`apn.unexpected_switch`).

### To confirm on the device

The steps are in DEVICE_CHECKLIST.md "APN profiles". The questions:

- Do both reads exist, with this shape, and does the list match the router's own APN page?
- Is the saved password returned in plain text, empty or masked?
- Are plain-XML add, edit, delete and switch accepted, and do they leave "In use" alone unless it's a switch?
- Does a wrong APN show as 902, 904 or 906 with no WAN address, so the automatic undo fires?

## 9. Privacy check reads (remote management and devices)

The paths are huawei-lte-api's (`Cwmp.basic_info`, `Lan.host_info` and `WLan.host_list` in `huawei_lte_api/api/`). On 2026-11-13 the H122 answered the first two, and the element names below are from that run's `privacy.checked` line, which holds names and no values. `wlan/host-list` hasn't been asked for. The bodies hold the provider's account for the router, device names and MAC addresses: they are parsed and dropped in the driver, never logged, and never part of the diagnostics export.

### Reads

- `GET /api/cwmp/basic-info`: the TR-069 (CWMP) settings. On the H122 its elements are `enable`, `inform`, `interval`, `informrangemin`, `informrangemax`, `acsurl`, `acsname`, `acspwd`, `conname`, `conpwd`, `conport`, `cert`, `iscwmpcertexisted`, `authmode` and `virtualserver_used_wanport`. The app looks for the server's address: a field whose name contains `url`, or any field whose value is an `http://` or `https://` address. A name containing `req` (the connection-request address is the router's own), `stun`, `user`, `pass` or `pwd` never counts, and a field named for the ACS wins over any other. From the value the app keeps the host and nothing else. Whether TR-069 is switched on is read from the element named exactly `enable` (1 or 0) and from nothing else.
- `GET /api/lan/HostInfo`: `<Hosts>` with one `<Host>` each. On the H122 a host has `ID`, `MacAddress`, `IpAddress`, `HostName`, `ActualName`, `Active`, `InterfaceType`, `AssociatedSsid`, `AssociatedTime`, `AddressSource`, `LeaseTime`, `isLocalDevice`, `IsBindIptvWan` and `EthernetSpeed`. The app reads `MacAddress`, `IpAddress`, `ActualName` (else `HostName`), `Active` (1 = online), `InterfaceType` (a word with wireless, wifi or wlan in it is Wi-Fi; ethernet, lan, wired or cable is cable; otherwise a non-empty `AssociatedSsid` is Wi-Fi) and `isLocalDevice` (1 is taken as the computer that asked, and shown only when exactly one host has it), compared without case. A host without a readable MAC address is skipped.
- `GET /api/wlan/host-list`: the devices on Wi-Fi now, in the same shape without `Active`. Asked only when the router refuses `lan/HostInfo` or answers it with something unreadable; a router that doesn't answer isn't asked twice.
- All three go through `HuaweiSession::get_optional`: an HTTP error status comes back with the status as the code, 404 as "not supported" and any other as a refusal, whatever the body holds.

### What the app logs

One `privacy.checked` event per press, with `remoteManagement` and `devices`: for each, `result` (`ok`, `refused`, `notOffered`, `unreadable`, `noAnswer` or `notSupported`), the router's `code` or the HTTP status when there was one, and `tags`, the answer's element names (40 at most, plain names only). `devices` also has `source` (`lan` or `wifi`, the list that answered), `lan` (how the LAN list ended, when the Wi-Fi list was used instead) and `wifi` (how the Wi-Fi list ended, when both failed). No values. The `tags` of the first real run are what confirm the names above.

### To confirm on the device

The steps are in DEVICE_CHECKLIST.md "Privacy check". The questions:

- What does `InterfaceType` hold for a Wi-Fi device and for a cabled one? Values aren't logged, so the card's Connection column is the check.
- Does `wlan/host-list` exist, and with which code when it does not?

### Not read yet: DNS, DMZ, firewall and MAC filter [huawei-lte-api, unverified]

Looked up on 2026-11-14. Nothing is built and nothing was sent. The paths and field names are from huawei-lte-api's `Dhcp.py`, `Security.py` and `WLan.py` (read 2026-11-14). The H122 may refuse them, as it refused the APN read and the data-switch write (§7).

- **DNS the router hands to devices:** `GET /api/dhcp/settings` has `DnsStatus`, `PrimaryDns` and `SecondaryDns` beside the DHCP range. Its write, `POST /api/dhcp/settings`, sends the whole block (`DhcpIPAddress`, `DhcpLanNetmask`, `DhcpStatus`, `DhcpStartIPAddress`, `DhcpEndIPAddress`, `DhcpLeaseTime`, `DnsStatus`, `PrimaryDns`, `SecondaryDns`, `ShowDnsSetting`), so a wrong value can take the local network down. This is not the DNS server Windows uses when the PC has its own setting; the card's DNS check shows that one.
- **Exposure to the internet:** `GET /api/security/dmz` (`DmzStatus`, `DmzIPAddress`), `GET /api/security/upnp` (`UpnpStatus`), `GET /api/security/firewall-switch` (`FirewallMainSwitch` and four more switches) and `GET /api/security/virtual-servers` (port forwards). The first three have a POST to the same path in that library; for `virtual-servers` it has only the read. Behind the operator's shared address (the H122's WAN address is in 10.x), a DMZ or a port forward lets nothing in from the internet over IPv4.
- **MAC filter:** `GET /api/security/mac-filter`, and for Wi-Fi `GET /api/wlan/multi-macfilter-settings` and `multi-macfilter-settings-ex`; the Wi-Fi write is a POST to `wlan/multi-macfilter-settings`.
- **Wi-Fi:** `GET /api/wlan/multi-security-settings` (the encryption of each network) and `GET /api/wlan/wps-switch`.

Reads would follow this section's pattern: an optional read with its own outcome and a names-only log line. They would raise the three-GETs-per-press limit that the tests hold. A write needs its exact call confirmed in this firmware's own page code first, and the privacy card never writes.
