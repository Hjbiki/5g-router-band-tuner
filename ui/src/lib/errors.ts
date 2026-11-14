import { t, type MessageKey } from './i18n'

/** Backend refusal strings (Rust) mapped to dictionary keys. */
const KNOWN: Record<string, MessageKey> = {
  'Finish or revert the current band change before changing router or driver.': 'settings.error.changeInProgress',
  'Wait for the IP refresh to finish before changing router or driver.': 'settings.error.ipRefreshInProgress',
  'Finish or undo the APN change before changing router or driver.': 'settings.error.apnInProgress',
  'invalid host': 'settings.error.invalidHost',
  'username must not be empty': 'settings.error.usernameEmpty',
  'no saved password': 'settings.error.noPassword',
  'saved password was rejected': 'settings.error.passwordRejected',
  'a band change is in progress': 'reboot.unavailable',
  'the reboot confirmation is invalid or expired': 'reboot.expired',
  'factory reset is not verified on this router model': 'danger.notVerified',
  'the factory reset confirmation is invalid or expired': 'danger.expired',
  'a speed test is running': 'danger.blockedSpeed',
  'an auto-scan is running': 'scan.blockedRunning',
  "couldn't reach GitHub": 'updates.unreachable',
  'pick at least one band to scan': 'scan.pickBand',
  'this scan was for another router': 'scan.otherRouter',
  'not connected to the router': 'bands.reason.notReady',
  'router service is not running': 'settings.error.serviceDown',
  'router service stopped': 'settings.error.serviceDown',
  "A recording of the router's page is open. Stop or discard it first.": 'errors.recordingOpen',
  'history is not available': 'errors.historyUnavailable',
  'history was made by a newer version of the app': 'errors.historyNewer',
  'wait for the auto-scan or speed test to finish': 'errors.historyBusy',
  'an IP refresh is running': 'ipRefresh.refuse.running',
  'the router has no service right now': 'ipRefresh.refuse.noService',
  'a reboot is waiting for confirmation': 'ipRefresh.refuse.rebootPending',
  "a mobile data record from before can't be used": 'ipRefresh.refuse.journalBlocked',
  'mobile data is still off': 'ipRefresh.refuse.stillOff',
  'mobile data is off on the router': 'network.apn.refuse.dataOff',
  'this router has no mobile data switch': 'ipRefresh.refuse.notSupported',
  'this router has no way to refresh the IP': 'ipRefresh.refuse.notSupported',
  'the address start must be one or two numbers from 0 to 255': 'ipRefresh.refuse.targetInvalid',
  'the number of tries must be from 1 to 5': 'ipRefresh.refuse.triesInvalid',
  'too many IP refreshes in the last hour': 'ipRefresh.refuse.cooldown',
  'too many band switches for a new IP in the last hour': 'ipReattach.refuse.cooldown',
  'the network mode has no 4G bands to switch': 'ipReattach.refuse.noLte',
  'there are no other bands to switch to for a moment': 'ipReattach.refuse.noOtherBands',
  'automatic undo is off': 'ipReattach.refuse.undoOff',
  'the address already starts with that': 'ipRefresh.refuse.alreadyMatches',
  'an APN change is in progress': 'network.apn.refuse.running',
  "an APN change record from before can't be used": 'network.apn.refuse.journalBlocked',
  'switching the APN is not supported on this router': 'network.apn.refuse.notSupported',
  'the router picks the APN by itself (Auto APN is on)': 'network.apn.refuse.autoOn',
  "that APN profile isn't on the router": 'network.apn.refuse.missing',
  'that APN profile changed on the router': 'network.apn.refuse.changed',
  'that APN profile is incomplete': 'network.apn.refuse.invalid',
  "the APN profiles haven't been read yet": 'network.apn.refuse.unknown',
  "that isn't an APN profile": 'network.apn.refuse.targetInvalid',
  'the router already has the requested configuration': 'network.apn.refuse.alreadyInUse',
  'the requested configuration is already active': 'network.apn.refuse.alreadyInUse',
  'editing APN profiles is not supported on this router': 'network.apn.edit.refuse.notSupported',
  'the APN in use changed on the router': 'network.apn.edit.refuse.currentMoved',
  'the router changed the active APN': 'network.apn.edit.refuse.routerSwitched',
  'that APN profile is in use': 'network.apn.edit.refuse.inUse',
  'that APN profile is set by the operator': 'network.apn.edit.refuse.readOnly',
  "the router refused the request; it may need an encrypted request, which the app can't send":
    'network.apn.edit.refuse.needsEncryption',
  "the router didn't save the profile as sent": 'network.apn.edit.refuse.saveMismatch',
  "the router hides this profile's saved password, so it can't be kept": 'network.apn.edit.refuse.passwordHidden',
  'a new profile has no saved password to keep': 'network.apn.edit.refuse.nothingToKeep',
  'the profile name must be 1 to 64 characters': 'network.apn.edit.refuse.nameInvalid',
  'the APN may hold only letters, digits, dots and dashes, up to 100': 'network.apn.edit.refuse.apnInvalid',
  'the user name must be at most 64 characters, without control characters': 'network.apn.edit.refuse.usernameInvalid',
  'the password must be 1 to 64 characters, without control characters': 'network.apn.edit.refuse.passwordInvalid',
  "that sign-in method or IP type can't be saved": 'network.apn.edit.refuse.kindInvalid',
  'Log in to the router first.': 'pageScripts.needLogin',
  'not logged in': 'pageScripts.needLogin',
  'Wait until the current band change, Refresh IP, APN change, auto-scan or speed test has finished.':
    'pageScripts.refuse.busy',
  "the router's page scripts are being read": 'pageScripts.refuse.reading',
  "The router's page scripts are already being read.": 'pageScripts.refuse.running',
  "Read the router's page scripts first.": 'pageScripts.refuse.notPrepared',
  "saving the router's page scripts is not supported": 'pageScripts.refuse.notSupported',
}

function raw(e: unknown): string {
  if (typeof e === 'string') return e
  if (e instanceof Error) return e.message
  if (e && typeof e === 'object') {
    const o = e as { message?: unknown; kind?: unknown }
    if (typeof o.message === 'string') return o.message
    if (typeof o.kind === 'string') return o.kind
  }
  return String(e)
}

/** The router's own code on a lost-session error, when it said so. */
function sessionLostCode(e: unknown): number | null | undefined {
  if (!e || typeof e !== 'object') return undefined
  const o = e as { kind?: unknown; code?: unknown }
  if (o.kind !== 'sessionLost') return undefined
  return typeof o.code === 'number' ? o.code : null
}

/** Turns a rejected command (string or RouterError object) into localised text. */
export function errorText(e: unknown): string {
  const code = sessionLostCode(e)
  if (code !== undefined) {
    return code === null ? t('errors.sessionLost') : t('errors.sessionLostCode', { code: String(code) })
  }
  const msg = raw(e)
  const key = KNOWN[msg]
  return key ? t(key) : t('app.error', { message: msg })
}
