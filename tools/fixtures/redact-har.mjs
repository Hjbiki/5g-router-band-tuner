#!/usr/bin/env node
// One-shot generator: turns a browser HAR capture of the Huawei web UI into
// redacted test fixtures. The HAR itself must never be committed.
//
// Usage: node tools/fixtures/redact-har.mjs <path-to-har> <output-dir>
//
// Every value of a sensitive tag/header is replaced by a deterministic synthetic
// value of the same length and character class. After writing, the script checks
// that no original secret value and nothing shaped like a real identifier or
// address appears in any output file, and exits non-zero if one does.

import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, writeFileSync, readdirSync } from "node:fs";
import { join } from "node:path";

const [harPath, outDir] = process.argv.slice(2);
if (!harPath || !outDir) {
  console.error("usage: redact-har.mjs <har> <out-dir>");
  process.exit(2);
}

const entries = JSON.parse(readFileSync(harPath, "utf8")).log.entries;

// Tags whose values are secret, identifying, or address-like.
const SENSITIVE_TAGS = new Set([
  "token", "salt", "servernonce", "firstnonce", "finalnonce", "clientproof",
  "serversignature", "rsapubkeysignature", "rsan",
  "SerialNumber", "Imei", "Imsi", "Iccid", "Msisdn", "MacAddress1", "MacAddress2",
  "WifiMacAddrWl0", "WifiMacAddrWl1", "WanIPAddress", "WanIPv6Address",
  "wan_dns_address", "wan_ipv6_dns_address", "submask",
  "PrimaryDns", "SecondaryDns", "PrimaryIPv6Dns", "SecondaryIPv6Dns",
]);
const TOKEN_HEADERS = /^__requestverificationtoken(one|two)?$/i;

const secrets = new Set(); // every original value we replaced, for the leak check
const generated = new Set(); // synthetic values we produced, exempt from the pattern scan
let counter = 0;

function synth(kind, len, label) {
  // Deterministic pseudo-random stream from the label, so reruns are stable.
  let out = "";
  let i = 0;
  while (out.length < len) {
    out += createHash("sha256").update(`fixture:${label}:${i++}`).digest("hex");
  }
  out = out.slice(0, len);
  if (kind === "digits") return out.replace(/[a-f]/g, c => String(c.charCodeAt(0) % 10));
  if (kind === "alnum") return out.replace(/[0-9a-f]/g, (c, k) => (k % 3 === 0 ? c.toUpperCase() : c));
  return out; // hex
}

function synthLike(value, label) {
  if (value === "") return "";
  secrets.add(value);
  const fake = synthLikeInner(value, label);
  generated.add(fake);
  return fake;
}

function synthLikeInner(value, label) {
  const n = ++counter;
  if (/^\d+\.\d+\.\d+\.\d+$/.test(value)) return `192.0.2.${n % 250 + 1}`; // RFC 5737 TEST-NET-1
  if (/^([0-9A-Fa-f]{2}[:-]){5}[0-9A-Fa-f]{2}$/.test(value)) return `02:00:00:00:00:${(n % 256).toString(16).padStart(2, "0").toUpperCase()}`;
  if (value.includes(":") && /^[0-9a-fA-F:.,;\s]+$/.test(value)) return "2001:db8::1"; // RFC 3849
  if (/^[\d.,;\s]+$/.test(value) && value.includes(".")) return "192.0.2.53,192.0.2.54";
  if (/^\d+$/.test(value)) return synth("digits", value.length, label);
  if (/^[0-9a-fA-F]+$/.test(value)) return synth("hex", value.length, label);
  return synth("alnum", value.length, label);
}

function redactXml(xml, label, fixed = {}) {
  return xml.replace(/<([A-Za-z_][\w.-]*)>([^<]*)<\/\1>/g, (m, tag, value) => {
    if (tag in fixed) {
      if (value) secrets.add(value);
      return `<${tag}>${fixed[tag]}</${tag}>`;
    }
    if (!SENSITIVE_TAGS.has(tag)) return m;
    return `<${tag}>${synthLike(value, `${label}:${tag}`)}</${tag}>`;
  });
}

function find(method, path, pred = () => true) {
  const e = entries.find(x => x.request.method === method && new URL(x.request.url).pathname === path && pred(x));
  if (!e) throw new Error(`not in HAR: ${method} ${path}`);
  return e;
}
const body = e => e.response.content.text || "";

mkdirSync(outDir, { recursive: true });
const written = [];
function out(name, content) {
  writeFileSync(join(outDir, name), content.endsWith("\n") ? content : content + "\n");
  written.push(name);
}

// --- Login flow -----------------------------------------------------------
const stateOut = find("GET", "/api/user/state-login", x => body(x).includes("<State>-1</State>"));
const stateIn = find("GET", "/api/user/state-login", x => body(x).includes("<State>0</State>"));
out("state_login_logged_out.xml", redactXml(body(stateOut), "state_out"));
out("state_login_logged_in.xml", redactXml(body(stateIn), "state_in"));

out("webserver_token.xml", redactXml(body(find("GET", "/api/webserver/token")), "token"));

// SCRAM: keep the structural relations the driver checks
// (servernonce starts with firstnonce; finalnonce == servernonce).
const challenge = find("POST", "/api/user/challenge_login");
const auth = find("POST", "/api/user/authentication_login");
const realFirst = challenge.request.postData.text.match(/<firstnonce>([^<]*)/)[1];
const realServer = body(challenge).match(/<servernonce>([^<]*)/)[1];
const fakeFirst = synth("hex", realFirst.length, "firstnonce");
const fakeServer = fakeFirst + synth("alnum", realServer.length - realFirst.length, "servernonce-suffix");
secrets.add(realFirst).add(realServer);
const nonceFix = { firstnonce: fakeFirst, servernonce: fakeServer, finalnonce: fakeServer };

out("challenge_login_request.xml", redactXml(challenge.request.postData.text, "challenge_req", nonceFix));
out("challenge_login_response.xml", redactXml(body(challenge), "challenge_resp", nonceFix));
out("authentication_login_request.xml", redactXml(auth.request.postData.text, "auth_req", nonceFix));
out("authentication_login_response.xml", redactXml(body(auth), "auth_resp", nonceFix));

// Header shapes: token pool and the one/two variants, values replaced.
const headerShape = {};
for (const [label, e] of [["challenge_login", challenge], ["authentication_login", auth]]) {
  headerShape[label] = {};
  for (const h of e.response.headers.filter(h => TOKEN_HEADERS.test(h.name))) {
    secrets.add(h.value);
    headerShape[label][h.name] = h.value
      .split("#")
      .map((part, i) => { secrets.add(part); return synth("hex", part.length, `${label}:${h.name}:${i}`); })
      .join("#");
  }
}
out("token_headers.json", JSON.stringify({
  _note: "Synthetic values. Shapes match the H122-373 capture: authentication_login returns a '#'-joined pool of 32-char tokens plus 'one'/'two' headers.",
  ...headerShape,
}, null, 2));

// --- Polled / informational endpoints -----------------------------------------
out("heartbeat.xml", body(find("GET", "/api/user/heartbeat")));
out("monitoring_status.xml", redactXml(body(find("GET", "/api/monitoring/status")), "status"));
out("current_plmn.xml", body(find("GET", "/api/net/current-plmn")));
out("net_mode.xml", body(find("GET", "/api/net/net-mode")));
out("basic_information.xml", redactXml(body(find("GET", "/api/device/basic_information")), "basic"));
out("device_information.xml", redactXml(body(find("GET", "/api/device/information")), "info"));
out("host_info_ok.xml", body(find("POST", "/api/host/info")));
out("error_lan_wan_config.xml", body(find("GET", "/api/ntwk/lan-wan-config")));

// --- Leak check -------------------------------------------------------------
const longSecrets = [...secrets].filter(s => s && s.length >= 6);
const patterns = [
  [/\b\d{15}\b/, "15-digit number (IMEI/IMSI-like)"],
  [/\b\d{19,20}\b/, "19/20-digit number (ICCID-like)"],
  [/\b(?!02:00:00:00:00:)([0-9A-F]{2}:){5}[0-9A-F]{2}\b/i, "MAC address outside 02:00:00:00:00:xx"],
  [/\b(?!192\.0\.2\.)(?!0\.0\.0\.0)\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b/, "IPv4 outside 192.0.2.0/24"],
];
let leaks = 0;
for (const name of readdirSync(outDir)) {
  const text = readFileSync(join(outDir, name), "utf8");
  // The pattern scan ignores our own synthetic values and firmware version strings.
  let scanned = text.replace(/<(\w*[Vv]ersion)>[^<]*<\/\1>/g, "");
  for (const g of generated) if (g) scanned = scanned.split(g).join("");
  for (const s of longSecrets) if (text.includes(s)) { console.error(`LEAK in ${name}: original value of length ${s.length}`); leaks++; }
  for (const [re, what] of patterns) if (re.test(scanned)) { console.error(`SUSPICIOUS in ${name}: ${what}`); leaks++; }
}
if (leaks) process.exit(1);
console.log(`wrote ${written.length} fixtures to ${outDir}; leak check passed (${longSecrets.length} secret values checked)`);
