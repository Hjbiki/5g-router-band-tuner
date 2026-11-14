// The router's internet (WAN) address, in words. The full address is only ever shown on request;
// everywhere else it reads as its range, the first two numbers.
import type { WanKind } from './bindings/WanKind'
import type { MessageKey } from './i18n'

/** "10.45.x.x" from the range "10.45". */
export function rangeText(range: string): string {
  return `${range}.x.x`
}

/** A Refresh IP target with its tail: "10" reads "10.x.x.x", "10.45" reads "10.45.x.x". */
export function targetText(target: string): string {
  return target.includes('.') ? rangeText(target) : `${target}.x.x.x`
}

export function wanKindKey(kind: WanKind): MessageKey {
  return `network.kind.${kind}` as MessageKey
}

export function wanKindShortKey(kind: WanKind): MessageKey {
  return `network.kindShort.${kind}` as MessageKey
}
