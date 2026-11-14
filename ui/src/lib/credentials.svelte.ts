import { api } from './backend'
import type { CredentialStatus } from './bindings/CredentialStatus'

/**
 * Whether a password is saved for the configured account and whether the router rejected it.
 * `null` until known, or when the backend couldn't say.
 */
export const credentials: { status: CredentialStatus | null } = $state({ status: null })

/** Re-reads the status; an error leaves it unknown. */
export async function refreshCred(): Promise<void> {
  try {
    credentials.status = await api.credentialStatus()
  } catch {
    credentials.status = null
  }
}
