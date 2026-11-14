import type { EventEntry } from './bindings/EventEntry'
import type { HistoryOverview } from './bindings/HistoryOverview'
import type { SignalPoint } from './bindings/SignalPoint'
import type { Stat3 } from './bindings/Stat3'

/** An open history database with some of everything in it. */
export function overviewFixture(o: Partial<HistoryOverview> = {}): HistoryOverview {
  return {
    available: true,
    unavailableReason: null,
    readOnly: false,
    firstMs: 0,
    lastMs: 0,
    minutes: 1440,
    speedtests: 4,
    events: 20,
    dbBytes: 1_000_000,
    retentionDays: 90,
    recording: true,
    ...o,
  }
}

const stat = (avg: number | null, spread = 2): Stat3 =>
  avg === null ? { min: null, avg: null, max: null } : { min: avg - spread, avg, max: avg + spread }

/** One bucket of signal on B3 with 5G, all of it in service. */
export function pointFixture(o: Partial<SignalPoint> = {}): SignalPoint {
  return {
    tMs: 0,
    aliveS: 60,
    observedS: 60,
    serviceS: 60,
    ownChangeS: 0,
    lteRsrp: stat(-95),
    lteRsrq: stat(-11),
    lteSinr: stat(8),
    nrRsrp: stat(-88),
    nrRsrq: stat(null),
    nrSinr: stat(16),
    nrShare: 1,
    lteBand: 3,
    allowedBands: '4',
    cellChanges: 0,
    ...o,
  }
}

export { stat as statFixture }

/** A router restart, as one entry of a page of events. */
export function rebootEntry(atMs: number): EventEntry {
  return {
    kind: 'single',
    item: {
      atMs,
      kind: 'device.reboot',
      category: 'device',
      severity: 'warning',
      technical: false,
      key: 'events.kind.deviceReboot',
      params: {},
      detailKey: null,
    },
  }
}
