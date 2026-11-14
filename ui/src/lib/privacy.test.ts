import { describe, expect, it } from 'vitest'
import { deviceCounts, deviceKind, maskAddress, readFailure } from './privacy'
import type { LanDevice } from './bindings/LanDevice'

describe('maskAddress', () => {
  it('keeps only the start of an address', () => {
    const cases: [string, string][] = [
      ['203.0.113.5', '203.0.x.x'],
      ['8.8.8.8', '8.8.x.x'],
      ['2001:db8:85a3::8a2e:370:7334', '2001:db8:…'],
      ['2001:db8::5', '2001:db8:…'],
      ['not-an-address', '…'],
      ['', '…'],
      ['1.2.3', '…'],
    ]
    for (const [ip, want] of cases) expect(maskAddress(ip), ip).toBe(want)
  })
})

describe('readFailure', () => {
  it('is nothing for a read that worked', () => {
    expect(readFailure({ outcome: 'ok', code: null })).toBeNull()
  })

  it('names how a read failed, with the router code when there is one', () => {
    expect(readFailure({ outcome: 'refused', code: 100003 })).toEqual({
      key: 'privacy.read.refusedCode',
      params: { code: '100003' },
    })
    expect(readFailure({ outcome: 'refused', code: null })).toEqual({ key: 'privacy.read.refused' })
    expect(readFailure({ outcome: 'notOffered', code: 100002 })).toEqual({
      key: 'privacy.read.notOfferedCode',
      params: { code: '100002' },
    })
    expect(readFailure({ outcome: 'notOffered', code: null })).toEqual({ key: 'privacy.read.notOffered' })
    expect(readFailure({ outcome: 'unreadable', code: null })).toEqual({ key: 'privacy.read.unreadable' })
    expect(readFailure({ outcome: 'noAnswer', code: null })).toEqual({ key: 'privacy.read.noAnswer' })
    expect(readFailure({ outcome: 'notSupported', code: null })).toEqual({ key: 'privacy.read.notSupported' })
  })
})

describe('deviceCounts', () => {
  const device = (active: boolean | null, privateMac: boolean): LanDevice => ({
    name: null,
    mac: '00:00:5E:00:53:01',
    ip: null,
    active,
    privateMac,
    link: null,
    thisDevice: false,
  })

  it('counts online devices only when the router says who is online', () => {
    expect(deviceCounts([device(true, false), device(false, true), device(true, true)])).toEqual({
      total: 3,
      online: 2,
      privateMac: 2,
    })
    // The Wi-Fi list doesn't say: unknown, never "0 online".
    expect(deviceCounts([device(null, false), device(null, true)])).toEqual({ total: 2, online: null, privateMac: 1 })
    expect(deviceCounts([])).toEqual({ total: 0, online: null, privateMac: 0 })
  })
})

describe('deviceKind', () => {
  it('guesses what a device is from the name it gives the router', () => {
    const cases: [string | null, string][] = [
      ['iPhone', 'phone'],
      ['Ahmeds-iPhone-15', 'phone'],
      ['Galaxy-S23', 'phone'],
      ['Pixel-8', 'phone'],
      ['V2244', 'phone'],
      ['SM-A546B', 'phone'],
      ['RMX3085', 'phone'],
      ['iPad', 'tablet'],
      ['Galaxy Tab', 'tablet'],
      ['DESKTOP-4F7K2AB', 'computer'],
      ['Office PC', 'computer'],
      ['iMac', 'computer'],
      ['LAPTOP-9QX1', 'laptop'],
      ['MacBook-Pro', 'laptop'],
      ['Living-room TV', 'tv'],
      ['BRAVIA-4K', 'tv'],
      ['Chromecast', 'tv'],
      ['PS5-123', 'console'],
      ['XboxOne', 'console'],
      ['Quest-3', 'vr'],
      ['SlimeVR-Tracker', 'tracker'],
      ['slimevr', 'tracker'],
      ['EPSON-L3250', 'printer'],
      ['Printer', 'printer'],
      ['Apple-Watch', 'watch'],
      ['HomePod', 'speaker'],
      ['Tapo-C200', 'camera'],
      ['TP-Link-Extender', 'network'],
      ['ESP-1A2B3C', 'chip'],
      ['shelly-plug', 'chip'],
      ['wlan0', 'unknown'],
      ['', 'unknown'],
      [null, 'unknown'],
    ]
    for (const [name, want] of cases) expect(deviceKind(name), String(name)).toBe(want)
  })

  it('does not take a word inside another word for a device', () => {
    // "tv" inside a word, "pc" inside a word, and a 4-digit code that isn't a phone model.
    expect(deviceKind('stvincent')).toBe('unknown')
    expect(deviceKind('epcot')).toBe('unknown')
    expect(deviceKind('X1234')).toBe('unknown')
  })
})
