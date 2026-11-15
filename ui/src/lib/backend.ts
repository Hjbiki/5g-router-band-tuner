import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import type { Alert } from './bindings/Alert'
import type { ApnDraft } from './bindings/ApnDraft'
import type { ApnProfiles } from './bindings/ApnProfiles'
import type { ApplyPreview } from './bindings/ApplyPreview'
import type { AppSnapshot } from './bindings/AppSnapshot'
import type { AutoSpeedTestStatus } from './bindings/AutoSpeedTestStatus'
import type { BandOutcome } from './bindings/BandOutcome'
import type { CapturePreview } from './bindings/CapturePreview'
import type { CaptureRefusal } from './bindings/CaptureRefusal'
import type { CaptureStatus } from './bindings/CaptureStatus'
import type { CaptureTexts } from './bindings/CaptureTexts'
import type { Bucket } from './bindings/Bucket'
import type { CredentialStatus } from './bindings/CredentialStatus'
import type { CsvLabels } from './bindings/CsvLabels'
import type { DnsCheck } from './bindings/DnsCheck'
import type { EventPage } from './bindings/EventPage'
import type { EventQuery } from './bindings/EventQuery'
import type { ExportOptions } from './bindings/ExportOptions'
import type { ExportOutcome } from './bindings/ExportOutcome'
import type { HistoryOverview } from './bindings/HistoryOverview'
import type { HistoryRange } from './bindings/HistoryRange'
import type { LogoutOutcome } from './bindings/LogoutOutcome'
import type { NetConfig } from './bindings/NetConfig'
import type { ProbeView } from './bindings/ProbeView'
import type { RebootTicket } from './bindings/RebootTicket'
import type { ScanFile } from './bindings/ScanFile'
import type { ScanMode } from './bindings/ScanMode'
import type { Settings } from './bindings/Settings'
import type { SignalSeries } from './bindings/SignalSeries'
import type { UpdateView } from './bindings/UpdateView'
import type { OoklaDoc } from './bindings/OoklaDoc'
import type { PageScriptsSummary } from './bindings/PageScriptsSummary'
import type { PrivacyReport } from './bindings/PrivacyReport'
import type { SpeedTestEvent } from './bindings/SpeedTestEvent'
import type { SpeedTestResult } from './bindings/SpeedTestResult'

// Tauri maps camelCase JS argument names to the snake_case Rust parameters.
export const api = {
  getSettings: () => invoke<Settings>('get_settings'),
  saveSettings: (s: Settings) => invoke<void>('save_settings', { s }),
  saveCredentials: (pw: string) => invoke<void>('save_credentials', { password: pw }),
  forgetCredentials: () => invoke<void>('forget_credentials'),
  connect: () => invoke<void>('connect'),
  connectTrusting: () => invoke<void>('connect_trusting'),
  logout: () => invoke<LogoutOutcome>('logout'),
  /** Refresh IP: one try without a target, else up to `maxTries` until the range starts with it. */
  ipRefreshStart: (target: string | null, maxTries: number) => invoke<void>('ip_refresh_start', { target, maxTries }),
  ipRefreshStop: () => invoke<void>('ip_refresh_stop'),
  ipReattachStart: () => invoke<void>('ip_reattach_start'),
  dataSwitchOn: () => invoke<void>('data_switch_on'),
  /** The APN profiles from the last read (never with passwords). */
  apnProfiles: () => invoke<ApnProfiles | null>('apn_profiles'),
  /** Reads the APN profiles again; the next snapshot carries them. */
  apnRefresh: () => invoke<void>('apn_refresh'),
  /** Switches the APN in use through safe apply; refused when the profile changed since it was read. */
  apnSwitch: (index: number, fingerprint: string) => invoke<void>('apn_switch', { index, fingerprint }),
  /** The exact request an APN switch would send; no network call. */
  apnPreview: (index: number, fingerprint: string) => invoke<ApplyPreview>('apn_preview', { index, fingerprint }),
  /** Adds an APN profile; resolves to its index. The draft's password goes to the router only. */
  apnAdd: (draft: ApnDraft) => invoke<number>('apn_add', { draft }),
  /** Rewrites a profile that isn't in use; refused when it changed since it was read. */
  apnEdit: (index: number, fingerprint: string, draft: ApnDraft) =>
    invoke<void>('apn_edit', { index, fingerprint, draft }),
  /** Deletes a profile that isn't in use; refused when it changed since it was read. */
  apnDelete: (index: number, fingerprint: string) => invoke<void>('apn_delete', { index, fingerprint }),
  credentialStatus: () => invoke<CredentialStatus>('credential_status'),
  getSnapshot: () => invoke<AppSnapshot>('get_snapshot'),
  previewConfig: (c: NetConfig) => invoke<ApplyPreview>('preview_config', { cfg: c }),
  applyConfig: (c: NetConfig) => invoke<void>('apply_config', { cfg: c }),
  keepNow: () => invoke<void>('keep_now'),
  revertNow: () => invoke<void>('revert_now'),
  requestReboot: () => invoke<RebootTicket>('request_reboot'),
  confirmReboot: (id: string) => invoke<void>('confirm_reboot', { id }),
  requestFactoryReset: () => invoke<RebootTicket>('request_factory_reset'),
  confirmFactoryReset: (id: string) => invoke<void>('confirm_factory_reset', { id }),
  setVisible: (v: boolean) => invoke<void>('set_visible', { v }),
  openDeveloperPage: () => invoke<void>('open_developer_page'),
  prepareDiagnostics: () => invoke<string>('prepare_diagnostics'),
  saveDiagnostics: () => invoke<boolean>('save_diagnostics'),
  /** Reads the router's own web page code (GETs only) into a zip kept by the backend. */
  preparePageScripts: () => invoke<PageScriptsSummary>('prepare_page_scripts'),
  savePageScripts: () => invoke<boolean>('save_page_scripts'),
  /** Reads remote management and the device list from the router (GETs only). Nothing is saved. */
  privacyCheck: () => invoke<PrivacyReport>('privacy_check'),
  /** One ordinary name lookup through Windows: which DNS server answers this PC. */
  dnsCheck: () => invoke<DnsCheck>('dns_check'),
  // Speed test
  runSpeedTest: () => invoke<void>('run_speed_test'),
  cancelSpeedTest: () => invoke<void>('cancel_speed_test'),
  speedTestRunning: () => invoke<boolean>('speed_test_running'),
  speedTestHistory: () => invoke<SpeedTestResult[]>('speed_test_history'),
  autoSpeedTestStatus: () => invoke<AutoSpeedTestStatus>('auto_speed_test_status'),
  bandOutcomes: () => invoke<BandOutcome[]>('band_outcomes'),
  scanPeriods: () => invoke<[number, number][]>('scan_periods'),
  // Unsupported routers
  routerProbeStatus: () => invoke<ProbeView | null>('router_probe_status'),
  prepareRouterReport: () => invoke<string>('prepare_router_report'),
  openRouterIssue: () => invoke<void>('open_router_issue'),
  // Updates
  updateStatus: () => invoke<UpdateView>('update_status'),
  checkForUpdates: () => invoke<UpdateView>('check_for_updates'),
  dismissUpdate: (version: string) => invoke<UpdateView>('dismiss_update', { version }),
  openReleasePage: () => invoke<void>('open_release_page'),
  // Auto-scan
  getScan: () => invoke<ScanFile | null>('get_scan'),
  /** `combos` are band-number lists the user asked to test together, after the single bands. */
  startScan: (mode: ScanMode, bands: number[], repeats: number, combos: number[][] = []) =>
    invoke<void>('start_scan', { mode, bands, repeats, combos }),
  continueScan: () => invoke<void>('continue_scan'),
  stopScan: () => invoke<void>('stop_scan'),
  dismissScan: () => invoke<void>('dismiss_scan'),
  openOoklaTerms: (doc: OoklaDoc) => invoke<void>('open_ookla_terms', { doc }),
  // History
  historyOverview: () => invoke<HistoryOverview>('history_overview'),
  historySignal: (range: HistoryRange, bucket: Bucket = 'auto') =>
    invoke<SignalSeries>('history_signal', { range, bucket }),
  /** Oldest first. */
  historySpeedtests: (range: HistoryRange) => invoke<SpeedTestResult[]>('history_speedtests', { range }),
  historyEvents: (q: EventQuery) => invoke<EventPage>('history_events', { q }),
  clearHistory: () => invoke<void>('clear_history'),
  /** Opens a save dialog (or a folder picker for all four); `cancelled` when the user backs out. */
  exportHistoryCsv: (opts: ExportOptions, labels: CsvLabels) =>
    invoke<ExportOutcome>('export_history_csv', { opts, labels }),
  // Recording the router's own page
  /** Opens the router's page in the recording window. A reason comes back when it can't start. */
  captureStart: (texts: CaptureTexts) => invoke<CaptureRefusal | null>('capture_start', { texts }),
  captureStatus: () => invoke<CaptureStatus>('capture_status'),
  /** Ends the recording and returns what would be saved. Nothing is written. */
  captureStop: () => invoke<CapturePreview>('capture_stop'),
  /** Builds the file again, with or without the router's own scripts. */
  capturePreview: (includeCode: boolean) => invoke<CapturePreview>('capture_preview', { includeCode }),
  /** Opens a save dialog; false when the user backs out. After a save the recording is forgotten. */
  captureSave: () => invoke<boolean>('capture_save'),
  captureDiscard: () => invoke<void>('capture_discard'),
}

export function onSnapshot(cb: (s: AppSnapshot) => void): Promise<() => void> {
  return listen<AppSnapshot>('snapshot', (e) => cb(e.payload))
}

export function onAlert(cb: (a: Alert) => void): Promise<() => void> {
  return listen<Alert>('alert', (e) => cb(e.payload))
}

export function onRouterProbe(cb: (v: ProbeView | null) => void): Promise<() => void> {
  return listen<ProbeView | null>('router-probe', (e) => cb(e.payload))
}

/** The recording moved on: opened, stopped, saved, thrown away, or its window was closed. */
export function onCaptureStatus(cb: (s: CaptureStatus) => void): Promise<() => void> {
  return listen<CaptureStatus>('capture-status', (e) => cb(e.payload))
}

export function onUpdate(cb: (v: UpdateView) => void): Promise<() => void> {
  return listen<UpdateView>('update', (e) => cb(e.payload))
}

export function onScan(cb: (f: ScanFile | null) => void): Promise<() => void> {
  return listen<ScanFile | null>('scan', (e) => cb(e.payload))
}

/** The automatic speed-test schedule changed: next slot, last outcome, or what blocks it. */
export function onAutoSpeedTest(cb: (s: AutoSpeedTestStatus) => void): Promise<() => void> {
  return listen<AutoSpeedTestStatus>('auto-speedtest', (e) => cb(e.payload))
}

export function onHistoryCleared(cb: () => void): Promise<() => void> {
  return listen<null>('history-cleared', () => cb())
}

export function onSpeedTest(cb: (e: SpeedTestEvent) => void): Promise<() => void> {
  return listen<SpeedTestEvent>('speedtest', (e) => cb(e.payload))
}
