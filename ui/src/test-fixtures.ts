import type { AppSnapshot } from './lib/bindings/AppSnapshot'
import type { Settings } from './lib/bindings/Settings'

export const SUPPORTED_HEX = '7e2880800d5'

/** A logged-in snapshot with sensible defaults for component tests. */
export function snap(o: Partial<AppSnapshot> = {}): AppSnapshot {
  return {
    session: { kind: 'loggedIn' },
    device: { model: 'H122-373', firmware: '11.0.2.21', webUi: '1' },
    status: null,
    signal: null,
    config: { networkMode: { kind: 'auto' }, lteBands: '5', legacyBandRaw: '0', nrOption: 'saNsa' },
    capabilities: {
      supportedLte: SUPPORTED_HEX,
      modes: [{ kind: 'auto' }, { kind: 'lteOnly' }, { kind: 'nrOnly' }],
      nrOptions: ['nsa', 'sa', 'saNsa'],
      bandEditAllowed: true,
      factoryResetVerified: false,
      dataSwitch: false,
      ipMethods: [],
      apn: { read: true, switch: false, edit: false, confirmed: false },
    },
    safeApply: { phase: { kind: 'idle' }, secondsLeft: null, previous: null, requested: null },
    inService: true,
    foreignJournal: null,
    capturedAtMs: 0,
    dataSwitchOff: false,
    ...o,
  }
}

/** Default settings as the backend sends them; override fields per test. */
export function settingsFixture(o: Partial<Settings> = {}): Settings {
  return {
    host: '192.168.8.1',
    driver: 'huawei_h122',
    username: 'admin',
    timers: { graceS: 90, lossS: 60, probationS: 180 },
    presets: [],
    language: 'en',
    digits: 'western',
    theme: 'system',
    palette: 'carbon',
    design: 'classic',
    uiScale: 100,
    cellAlerts: true,
    startAtLogin: false,
    trayNoticeShown: false,
    onboardingDone: false,
    automation: { primaryBand: null, relockPrimary: false, revertOnLoss: true },
    speedTest: { provider: 'cloudflare', size: 'light', ooklaTermsAccepted: false },
    updateCheck: false,
    history: {
      retentionDays: 90,
      autoSpeedTest: { enabled: false, everyH: 3, provider: 'cloudflare', size: 'light' },
    },
    ipRefresh: { lastTarget: null, maxTries: 5 },
    ...o,
  }
}
