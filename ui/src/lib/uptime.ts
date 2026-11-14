import { formatNumber, type DigitStyle } from './format'

// The router status carries no uptime, so this is how long this app has been logged in.
// Shared, so the sidebar and Settings show the same clock.
let since: number | null = null

/** "hh:mm:ss" since the login began, or null while logged out. */
export function loginUptime(loggedIn: boolean, now: number, digits: DigitStyle): string | null {
  if (!loggedIn) {
    since = null
    return null
  }
  since ??= now
  const s = Math.max(0, Math.floor((now - since) / 1000))
  const two = (n: number) => formatNumber(Math.floor(n / 10), { digits }) + formatNumber(n % 10, { digits })
  return `${two(Math.floor(s / 3600))}:${two(Math.floor((s % 3600) / 60))}:${two(s % 60)}`
}
