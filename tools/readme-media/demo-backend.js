// Stands in for the Tauri backend while recording README media. Everything is made up, following
// the demo driver (crates/router-core/src/drivers/mock.rs). Never put real router values here.
const q = new URLSearchParams(location.search)
const scene = q.get('scene') ?? 'signal'

const NR_ARFCN = 520110
const cfgAuto = { networkMode: { kind: 'auto' }, lteBands: '5', legacyBandRaw: '0', nrOption: 'nsa' }

const jitter = (v, d) => Math.round((v + (Math.random() - 0.5) * d) * 10) / 10
const lteSig = () => ({
  band: 1, earfcnDl: 300, earfcnUl: 18300, pci: 101, cellId: 1000001, enbId: null, sector: null,
  rsrp: jitter(-95, 2), rsrq: jitter(-11, 1), sinr: jitter(8, 3), rssi: jitter(-68, 2), txPower: 12,
  dlBwMhz: 20, ulBwMhz: 20, dlMcs: [22, 18], ulMcs: 14, cqi: [11, 10], sccPci: null,
})
const nrSig = () => ({
  arfcnDl: NR_ARFCN, arfcnUl: NR_ARFCN, band: { primary: 41, alternates: [], duplex: 'tdd', dlMhz: 2500 }, pci: 101,
  rsrp: jitter(-88, 2), rsrq: jitter(-10, 1), sinr: jitter(16, 3), txPower: 17, dlBwMhz: 100, ulBwMhz: 100,
  dlMcs: [24, 20], ulMcs: 16, cqi: [13, 12],
})

let phase = { kind: 'idle' }
let secondsLeft = null
let inService = true
let config = cfgAuto
let requested = null
let previous = null

const snapshot = () => ({
  session: { kind: 'loggedIn' },
  device: { model: 'Mock H122-373', firmware: 'mock-11.0.2.21', webUi: 'mock', homePlmn: null },
  status: {
    connection: { kind: inService ? 'connected' : 'noService' }, networkType: inService ? 20 : 19,
    operator: null, bars: inService ? 4 : null, nrBars: inService ? 4 : null,
  },
  signal: inService ? { lte: lteSig(), nr: nrSig(), serving: 'lte' } : { lte: null, nr: null, serving: null },
  config,
  capabilities: {
    supportedLte: '7e2880800d5',
    modes: [{ kind: 'auto' }, { kind: 'nrOnly' }, { kind: 'lteOnly' }],
    nrOptions: ['nsa', 'sa', 'saNsa'], bandEditAllowed: true, factoryResetVerified: true, dataSwitch: false,
    apn: { read: true, switch: false, edit: false, confirmed: true },
  },
  safeApply: { phase, secondsLeft, previous, requested, policy: { revertOnLoss: true, lossIgnored: false }, sequence: null },
  inService,
  foreignJournal: null,
  capturedAtMs: Date.now(),
  dataSwitchOff: false,
  wan: { ipv4: '10.45.37.142', range: '10.45', kind: 'private', ipv6: false },
  apn: {
    current: 1,
    autoApn: false,
    profiles: [
      { index: 1, name: 'Operator internet', apn: 'internet.example', apnStatic: true, username: '', hasPassword: false, auth: 'auto', ipType: 'ipv4v6', readOnly: true, valid: true, fingerprint: '0123456789abcdef' },
      { index: 2, name: 'Work', apn: 'work.example', apnStatic: true, username: '', hasPassword: false, auth: 'auto', ipType: 'ipv4v6', readOnly: false, valid: true, fingerprint: 'fedcba9876543210' },
    ],
  },
})

const settings = {
  updateCheck: false, host: '192.168.8.1', driver: 'mock', username: 'admin',
  timers: { graceS: 90, lossS: 60, probationS: 180 }, presets: [],
  language: q.get('lang') ?? 'en', digits: 'western', theme: 'dark', palette: 'carbon', design: q.get('design') ?? 'classic',
  uiScale: 100, cellAlerts: true, startAtLogin: false, trayNoticeShown: true, onboardingDone: true,
  automation: { primaryBand: 1, relockPrimary: false, revertOnLoss: true },
  speedTest: { provider: 'cloudflare', size: 'light', ooklaTermsAccepted: false },
  history: {
    retentionDays: 90,
    autoSpeedTest: { enabled: false, everyH: 3, provider: 'cloudflare', size: 'light' },
  },
  ipRefresh: { lastTarget: null, maxTries: 5 },
}

const serving = { rat: 'lte', lteBand: 1, ltePci: 101, lteEarfcn: 300, lteCa: false, nrBand: 41, nrPci: 101, nrArfcn: NR_ARFCN }
const past = (hours, o) => ({
  atMs: Date.now() - hours * 3600_000, provider: 'cloudflare', requested: 'cloudflare', fallback: null, fallbackDetail: null,
  size: 'light', serving, idlePingMs: 26, downloadMbps: 172, uploadMbps: 21, pingDownMs: 190, pingUpMs: 240,
  bytesUsed: 40000000, server: 'Cloudflare', ...o,
})
const spread = (r) => r.downloadMbps == null ? r : ({ ...r, downloadMinMbps: Math.round(r.downloadMbps * 0.8), downloadMaxMbps: Math.round(r.downloadMbps * 1.2), uploadMinMbps: Math.round(r.uploadMbps * 8) / 10, uploadMaxMbps: Math.round(r.uploadMbps * 12) / 10 })
const row = (bands, o) => spread({
  atMs: 1, bands, isStart: false, outcome: 'measured', note: null, lteBand: 1, ltePci: 101, nrBand: 41, got5g: true,
  lteRsrp: -95, lteSinr: 8, nrSinr: 16, downloadMbps: 150, uploadMbps: 20, idlePingMs: 28, pingDownMs: 180, pingUpMs: 230,
  bytesUsed: 80000000, tests: 2, downloadMinMbps: null, downloadMaxMbps: null, uploadMinMbps: null, uploadMaxMbps: null, ...o,
})
const scanFile = () => scene !== 'scan' ? null : ({
  version: 1, host: settings.host, driver: 'mock', mode: 'quick', status: 'done', step: 'testing', start: cfgAuto,
  queue: ['5', '1', '4', '8000000', '8000000000'], next: 5, combosAdded: false, startedMs: 1, returnFailed: false, repeats: 2, test: 2, model: null, addOnBands: [], customSets: [],
  results: [
    row('5', { isStart: true, downloadMbps: 164, uploadMbps: 19 }),
    row('1', { downloadMbps: 186, uploadMbps: 23, pingDownMs: 150, pingUpMs: 200 }),
    row('4', { lteBand: 3, ltePci: 103, lteRsrp: -100, lteSinr: 0, nrSinr: 9, downloadMbps: 92, uploadMbps: 14, pingDownMs: 260, pingUpMs: 340 }),
    row('8000000', { lteBand: 28, ltePci: 128, lteRsrp: -82, lteSinr: 18, nrBand: null, got5g: false, nrSinr: null, downloadMbps: 48, uploadMbps: 11, pingDownMs: 95, pingUpMs: 130 }),
    row('8000000000', { outcome: 'noService', lteBand: null, ltePci: null, nrBand: null, got5g: false, lteRsrp: null, lteSinr: null, nrSinr: null, downloadMbps: null, uploadMbps: null, idlePingMs: null, pingDownMs: null, pingUpMs: null, bytesUsed: 0 }),
  ],
})

// History: a made-up 30-day record ending now, the same on every call. The app wasn't running on
// some nights (gaps), the allowed bands moved from B1+B3 to B1 a week ago, and 5G drops out now and
// then. Demo PCIs only.
const MIN = 60_000
const HOUR = 60 * MIN
const DAY = 24 * HOUR
const histEnd = Math.floor(Date.now() / MIN) * MIN
const histStart = histEnd - 30 * DAY
const wave = (t, period, phase = 0) => Math.sin((2 * Math.PI * t) / period + phase)
const off = (t) => { const h = new Date(t).getHours(); const day = Math.floor((t - histStart) / DAY); return day % 4 === 1 && h >= 2 && h < 6 }
const allowedAt = (t) => (t < histEnd - 7 * DAY ? '5' : '1')
const NONE = { min: null, avg: null, max: null }
const stat = (avg, spread) => avg === null ? NONE : ({ min: Math.round((avg - spread) * 10) / 10, avg: Math.round(avg * 10) / 10, max: Math.round((avg + spread) * 10) / 10 })
const BUCKETS = [['min1', MIN], ['min5', 5 * MIN], ['min15', 15 * MIN], ['hour1', HOUR], ['hour6', 6 * HOUR], ['day1', DAY]]
function historySignal({ range, bucket }) {
  const span = range.toMs - range.fromMs
  const pick = bucket === 'auto' ? BUCKETS.find(([, ms]) => span / ms <= 800) ?? BUCKETS.at(-1) : BUCKETS.find(([b]) => b === bucket)
  const [name, ms] = pick
  const points = []
  for (let t = Math.ceil(Math.max(range.fromMs, histStart) / ms) * ms; t <= Math.min(range.toMs, histEnd); t += ms) {
    if (off(t)) continue
    const busy = wave(t, DAY, -1.2)
    const nr = wave(t, 5 * HOUR) > -0.7
    const spread = Math.min(4, 1 + ms / HOUR)
    const s = ms / 1000
    points.push({
      tMs: t, aliveS: s, observedS: s, serviceS: wave(t, 3 * DAY) < -0.98 ? s * 0.6 : s, ownChangeS: 0,
      lteRsrp: stat(-95 + 3 * busy, spread), lteRsrq: stat(-11 + busy, spread / 2), lteSinr: stat(8 - 3 * busy + wave(t, 7 * HOUR), spread),
      nrRsrp: nr ? stat(-88 + 2 * busy, spread) : stat(null, 0), nrRsrq: nr ? stat(-10, 1) : stat(null, 0),
      nrSinr: nr ? stat(16 - 4 * busy, spread) : stat(null, 0),
      nrShare: nr ? 1 : 0, lteBand: 1, allowedBands: allowedAt(t), cellChanges: 0,
    })
  }
  return { bucket: name, bucketMs: ms, points }
}
const SOURCES = ['auto', 'auto', 'manual', 'scan']
const histSpeeds = (() => {
  const out = []
  for (let t = histStart + 5 * HOUR, i = 0; t < histEnd; t += 8 * HOUR + (i % 3) * HOUR, i++) {
    if (off(t)) continue
    const busy = wave(t, DAY, -1.2)
    out.push(past(0, {
      atMs: t, source: SOURCES[i % SOURCES.length], allowedBands: allowedAt(t),
      downloadMbps: Math.round((150 - 70 * busy + 20 * wave(t, 11 * HOUR)) * 10) / 10,
      uploadMbps: Math.round((20 - 6 * busy) * 10) / 10,
      pingDownMs: Math.round(190 + 60 * busy), pingUpMs: Math.round(240 + 80 * busy),
    }))
  }
  return out
})()
const item = (atMs, kind, category, severity, key, params = {}) => ({ atMs, kind, category, severity, technical: false, key, params, detailKey: null })
const histEvents = (() => {
  const out = []
  for (let d = 0; d < 30; d++) {
    const t = histStart + d * DAY + 6 * HOUR + 7 * MIN
    if (d % 4 === 2) out.push({ kind: 'single', item: item(t, 'app.started', 'app', 'info', 'events.kind.appStarted') })
  }
  const change = (t, from, to, outcome) => ({ kind: 'change', group: {
    startedMs: t, endedMs: t + 4 * MIN, from, to, primary: null, outcome,
    steps: [item(t, 'safe_apply.started', 'bandChange', 'info', 'events.kind.safeApplyStarted', { bands: { t: 'bands', v: to } })],
  } })
  out.push(change(histEnd - 7 * DAY, [1, 3], [1], { kind: 'kept' }))
  out.push(change(histEnd - 12 * DAY, [1, 3], [28], { kind: 'reverted', reason: 'safeApply.reason.connectionLost', fallback: false }))
  out.push(change(histEnd - 2 * DAY, [1], [3], { kind: 'reverted', reason: 'safeApply.reason.connectionLost', fallback: false }))
  out.push({ kind: 'single', item: item(histEnd - 20 * DAY, 'device.reboot', 'device', 'info', 'events.kind.deviceReboot') })
  const at = (e) => (e.kind === 'change' ? e.group.startedMs : e.item.atMs)
  return out.sort((a, b) => at(b) - at(a))
})()
const inRange = (range) => (t) => t >= range.fromMs && t <= range.toMs
const historyOverview = () => ({
  available: true, unavailableReason: null, readOnly: false, firstMs: histStart, lastMs: histEnd,
  minutes: 27 * 24 * 60, speedtests: histSpeeds.length, events: histEvents.length, dbBytes: 18_400_000,
  retentionDays: 90, recording: true,
})

const listeners = {}
const emit = (name, payload) => (listeners[name] ?? []).forEach((cb) => cb({ payload }))

export async function invoke(cmd, args = {}) {
  switch (cmd) {
    case 'get_settings': return settings
    case 'get_snapshot': return snapshot()
    case 'speed_test_history': return scene === 'speed' ? [past(3, {}), past(27, { downloadMbps: 58, uploadMbps: 9, serving: { ...serving, lteBand: 28, ltePci: 128, nrBand: null, nrPci: null, nrArfcn: null } })] : []
    case 'speed_test_running': return false
    case 'band_outcomes': return []
    case 'scan_periods': return []
    case 'credential_status': return { saved: true, rejected: false }
    case 'get_scan': return scanFile()
    case 'router_probe_status': return null
    case 'update_status': return { current: '0.0.0', available: null, lastCheckMs: null }
    case 'preview_config': return { fields: [['LTEBand', '5']], body: '<request/>' }
    case 'history_overview': return historyOverview()
    case 'history_signal': return historySignal(args)
    case 'history_speedtests': return histSpeeds.filter((r) => inRange(args.range)(r.atMs))
    case 'history_events': {
      const keep = inRange(args.q.range)
      const before = args.q.beforeMs ?? Infinity
      const at = (e) => (e.kind === 'change' ? e.group.startedMs : e.item.atMs)
      return { entries: histEvents.filter((e) => keep(at(e)) && at(e) < before), nextBeforeMs: null }
    }
    case 'auto_speed_test_status': return { enabled: false, nextSlotMs: null, lastAttemptMs: null, lastOutcome: null, lastSkipReason: null, blockedBy: null, running: false }
    default: return undefined
  }
}
export const listen = async (name, cb) => {
  ;(listeners[name] ??= []).push(cb)
  return () => {}
}
export const getVersion = async () => '0.0.0'
export const getCurrentWebview = () => ({ setZoom: async () => {} })
export const getCurrentWindow = () => new Proxy({}, { get: () => async () => () => {} })
export const Window = {}

const wait = (ms) => new Promise((r) => setTimeout(r, ms))
const tick = () => emit('snapshot', snapshot())

async function bandsTimeline() {
  previous = cfgAuto
  requested = { ...cfgAuto, lteBands: '1' }
  const set = async (p, left, ms, up = inService) => {
    phase = p; secondsLeft = left; inService = up; tick(); await wait(ms)
  }
  await set({ kind: 'applying' }, null, 700, false)
  config = requested
  await set({ kind: 'verifying' }, null, 700, false)
  for (const s of [88, 87, 86]) await set({ kind: 'reattachGrace' }, s, 1000, false)
  for (const s of [180, 179, 178, 177]) await set({ kind: 'probation' }, s, 1000, true)
  await set({ kind: 'committed' }, null, 2000, true)
}

async function speedTimeline() {
  const live = { idlePingMs: null, downloadMbps: null, uploadMbps: null, pingDownMs: null, pingUpMs: null }
  emit('speedtest', { kind: 'started', provider: 'cloudflare', size: 'light', serving, sizeApplies: true })
  const step = async (p, ms, n, f) => {
    for (let i = 1; i <= n; i++) {
      f(i / n)
      emit('speedtest', { kind: 'progress', phase: p, fraction: i / n, bytesUsed: Math.round((p === 'upload' ? 30e6 : 0) + (i / n) * (p === 'upload' ? 10e6 : 30e6)), live: { ...live } })
      await wait(ms / n)
    }
  }
  await step('idlePing', 900, 3, () => { live.idlePingMs = 24 })
  await step('download', 3400, 14, (x) => {
    live.downloadMbps = Math.round((60 + 125 * Math.pow(x, 0.5) + Math.random() * 8) * 10) / 10
    live.pingDownMs = Math.round(120 + 70 * x)
  })
  await step('upload', 2600, 11, (x) => {
    live.uploadMbps = Math.round((6 + 16 * Math.pow(x, 0.6) + Math.random() * 2) * 10) / 10
    live.pingUpMs = Math.round(150 + 90 * x)
  })
  emit('speedtest', { kind: 'finished', ...past(0, { idlePingMs: 24, downloadMbps: 184.6, uploadMbps: 22.4, pingDownMs: 188, pingUpMs: 236 }) })
}

window.__go = () => {
  const pulse = setInterval(() => { if (scene !== 'bands') tick() }, 1000)
  if (scene === 'bands') bandsTimeline()
  if (scene === 'speed') speedTimeline()
  setTimeout(() => clearInterval(pulse), 20000)
}
