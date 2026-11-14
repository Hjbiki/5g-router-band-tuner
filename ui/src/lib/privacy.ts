// The read-only privacy check, in words. What it reads is kept in the open page only.
import type { LanDevice } from './bindings/LanDevice'
import type { PrivacyRead } from './bindings/PrivacyRead'
import type { MessageKey } from './i18n'

/** An address cut to its start: "203.0.x.x", or "2001:db8:…" for IPv6. Anything else reads "…". */
export function maskAddress(ip: string): string {
  const v4 = /^(\d{1,3})\.(\d{1,3})\.\d{1,3}\.\d{1,3}$/.exec(ip)
  if (v4) return `${v4[1]}.${v4[2]}.x.x`
  const v6 = /^([0-9a-f]{1,4}):([0-9a-f]{1,4}):[0-9a-f:.]*$/i.exec(ip)
  if (v6) return `${v6[1]}:${v6[2]}:…`
  return '…'
}

export interface ReadFailure {
  key: MessageKey
  params?: Record<string, string>
}

/** The sentence for a read that didn't work, with the router's own code when it gave one. */
export function readFailure(read: PrivacyRead): ReadFailure | null {
  if (read.outcome === 'ok') return null
  if (read.code !== null && (read.outcome === 'refused' || read.outcome === 'notOffered')) {
    return { key: `privacy.read.${read.outcome}Code` as MessageKey, params: { code: String(read.code) } }
  }
  return { key: `privacy.read.${read.outcome}` as MessageKey }
}

/** `online` is null when the router doesn't say which devices are online (never a made-up 0). */
export function deviceCounts(devices: LanDevice[]): { total: number; online: number | null; privateMac: number } {
  const known = devices.filter((d) => d.active !== null)
  return {
    total: devices.length,
    online: known.length ? known.filter((d) => d.active).length : null,
    privateMac: devices.filter((d) => d.privateMac).length,
  }
}

export type DeviceKind =
  | 'phone'
  | 'tablet'
  | 'computer'
  | 'laptop'
  | 'tv'
  | 'console'
  | 'vr'
  | 'tracker'
  | 'printer'
  | 'watch'
  | 'speaker'
  | 'camera'
  | 'network'
  | 'chip'
  | 'unknown'

// Checked in order, the more specific words first: "Galaxy Tab" is a tablet before it is a
// Galaxy phone, and "LAPTOP-…" a laptop before "PC" can claim it. The last entries of the phone
// line are makers' model codes (vivo V2244, Samsung SM-A546B, realme RMX3085, OPPO CPH2591).
const KIND_WORDS: [DeviceKind, RegExp][] = [
  ['tracker', /slime-?vr|tracker/],
  ['vr', /quest|oculus|\bpico\b|\bvive\b|valve-?index|\bvr\b/],
  ['tablet', /ipad|tablet|\btab\b|matepad|mediapad/],
  ['watch', /watch/],
  ['laptop', /laptop|macbook|notebook|thinkpad|matebook|zenbook|vivobook|ideapad|surface/],
  ['tv', /\btv\b|bravia|chromecast|roku|fire-?(tv|stick)|apple-?tv|webos|tizen|smart-?tv|mi-?box/],
  ['console', /playstation|\bps[345]\b|xbox|nintendo|steam-?deck/],
  ['printer', /printer|epson|canon|brother|laserjet|officejet|deskjet/],
  ['speaker', /\becho\b|alexa|homepod|google-?home|nest-?(mini|hub|audio)|sonos|speaker/],
  ['camera', /camera|\bcam\b|ipcam|doorbell|tapo|ezviz|hikvision|dahua/],
  ['network', /router|extender|repeater|\bmesh\b|access-?point|\bdeco\b|\beero\b|\borbi\b|openwrt/],
  ['chip', /\besp\b|esp32|esp8266|espressif|tasmota|shelly|sonoff|tuya|arduino|raspberry|wled|\bplug\b|bulb/],
  ['computer', /desktop|\bpc\b|imac|mac-?mini|mac-?studio|workstation/],
  [
    'phone',
    /iphone|android|galaxy|pixel|redmi|xiaomi|\bpoco\b|honor|oppo|vivo|realme|oneplus|nokia|phone|^v\d{4}[a-z]?$|^sm-[a-z]\d{3}|^rmx\d{4}|^cph\d{4}/,
  ],
]

/**
 * A guess at what a device is, from the name it gave the router, for its picture only. A name is
 * whatever the device chose, so this decides nothing else, and no name is 'unknown'.
 */
export function deviceKind(name: string | null): DeviceKind {
  const words = (name ?? '').toLowerCase().replace(/_/g, '-')
  if (!words) return 'unknown'
  return KIND_WORDS.find(([, pattern]) => pattern.test(words))?.[0] ?? 'unknown'
}
