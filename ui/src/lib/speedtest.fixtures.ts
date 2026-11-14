import type { SpeedTestResult } from './bindings/SpeedTestResult'

/** A synthetic speed-test result for tests (B3/PCI 103 + n41, slow upload). */
export function resultFixture(o: Partial<SpeedTestResult> = {}): SpeedTestResult {
  return {
    atMs: 1_000,
    provider: 'cloudflare',
    requested: 'cloudflare',
    fallback: null,
    fallbackDetail: null,
    size: 'light',
    serving: {
      rat: 'lte',
      lteBand: 3,
      ltePci: 103,
      lteEarfcn: 1300,
      lteCa: false,
      nrBand: 41,
      nrPci: 100,
      nrArfcn: 528980,
    },
    idlePingMs: 41,
    downloadMbps: 108,
    uploadMbps: 1.3,
    pingDownMs: 610,
    pingUpMs: 1450,
    bytesUsed: 38_000_000,
    server: 'Cloudflare JED',
    ...o,
  }
}
