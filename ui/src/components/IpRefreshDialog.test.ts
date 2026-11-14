import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/svelte'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))
vi.mock('../lib/backend', () => ({
  api: { ipRefreshStart: vi.fn() },
}))

import { api } from '../lib/backend'
import { app } from '../lib/store.svelte'
import { setLanguage, t } from '../lib/i18n'
import { ipRefreshUi, openIpRefresh, closeIpRefresh, normalizeTarget, estimateSeconds } from '../lib/ipRefresh.svelte'
import { settingsFixture, snap } from '../test-fixtures'
import IpRefreshDialog from './IpRefreshDialog.svelte'
import type { AppSnapshot } from '../lib/bindings/AppSnapshot'

const WAN = { ipv4: '10.45.37.142', range: '10.45', kind: 'private', ipv6: false } as const
const ready = (o: Partial<AppSnapshot> = {}): AppSnapshot => {
  const s = snap({ wan: WAN, ...o })
  if (s.capabilities) s.capabilities = { ...s.capabilities, dataSwitch: true, ipMethods: ['dataSwitch'] }
  return s
}

const flush = () => new Promise((r) => setTimeout(r, 0))

describe('IpRefreshDialog', () => {
  beforeEach(() => {
    vi.mocked(api.ipRefreshStart).mockResolvedValue(undefined)
    app.settings = settingsFixture({ ipRefresh: { lastTarget: '100.70', maxTries: 3 } })
    app.snapshot = ready()
  })
  afterEach(() => {
    cleanup()
    closeIpRefresh()
    vi.clearAllMocks()
  })

  it('starts one try without a target', async () => {
    render(IpRefreshDialog)
    openIpRefresh()
    await flush()
    expect(screen.getByRole('dialog')).toBeTruthy()
    const aim = screen.getByTestId('ipr-aim') as HTMLInputElement
    expect(aim.checked).toBe(false)
    // The last target is filled in, ready to tick.
    expect((screen.getByTestId('ipr-target') as HTMLInputElement).value).toBe('100.70')
    expect(screen.getByText(t('ipRefresh.dialog.oneTry'))).toBeTruthy()
    await fireEvent.click(screen.getByTestId('ipr-start'))
    expect(api.ipRefreshStart).toHaveBeenCalledWith(null, 1)
    await flush()
    expect(ipRefreshUi.open).toBe(false)
  })

  it('checks the address start and limits the tries to 5', async () => {
    render(IpRefreshDialog)
    openIpRefresh('10.62')
    await flush()
    expect((screen.getByTestId('ipr-aim') as HTMLInputElement).checked).toBe(true)
    const input = screen.getByTestId('ipr-target') as HTMLInputElement
    expect(input.value).toBe('10.62')
    const start = screen.getByTestId('ipr-start') as HTMLButtonElement

    for (const bad of ['10.45.12', '256', 'abc', '']) {
      await fireEvent.input(input, { target: { value: bad } })
      expect(start.disabled, bad).toBe(true)
      expect(input.getAttribute('aria-invalid')).toBe('true')
      expect(screen.getByText(t('ipRefresh.refuse.targetInvalid'))).toBeTruthy()
    }
    await fireEvent.input(input, { target: { value: '100.' } })
    expect(start.disabled).toBe(false)

    // From the saved 3 up to 5, never 6.
    expect(screen.getByTestId('ipr-tries').textContent).toBe('3')
    const more = screen.getByRole('button', { name: t('ipRefresh.dialog.more') }) as HTMLButtonElement
    for (let i = 0; i < 4; i++) await fireEvent.click(more)
    expect(screen.getByTestId('ipr-tries').textContent).toBe('5')
    expect(more.disabled).toBe(true)
    const fewer = screen.getByRole('button', { name: t('ipRefresh.dialog.fewer') }) as HTMLButtonElement
    for (let i = 0; i < 6; i++) await fireEvent.click(fewer)
    expect(screen.getByTestId('ipr-tries').textContent).toBe('1')
    expect(fewer.disabled).toBe(true)
    await fireEvent.click(more)

    await fireEvent.click(start)
    expect(api.ipRefreshStart).toHaveBeenCalledWith('100', 2)
  })

  it('says inline why it cannot start, and shows a refusal from the backend', async () => {
    app.snapshot = ready({ safeApply: { phase: { kind: 'probation' }, secondsLeft: 100, previous: null, requested: null } })
    render(IpRefreshDialog)
    openIpRefresh()
    await flush()
    expect(screen.getByTestId('ipr-refusal').textContent).toBe(t('ipRefresh.why.bandChange'))
    expect((screen.getByTestId('ipr-start') as HTMLButtonElement).disabled).toBe(true)

    app.snapshot = ready()
    await flush()
    expect(screen.queryByTestId('ipr-refusal')).toBeNull()
    vi.mocked(api.ipRefreshStart).mockRejectedValue({ kind: 'validation', message: 'too many IP refreshes in the last hour' })
    await fireEvent.click(screen.getByTestId('ipr-start'))
    await flush()
    expect(screen.getByTestId('ipr-error').textContent).toBe(t('ipRefresh.refuse.cooldown'))
    expect(ipRefreshUi.open).toBe(true)
  })

  it('is refused without the switch or service', async () => {
    app.snapshot = ready({ inService: false })
    render(IpRefreshDialog)
    openIpRefresh()
    await flush()
    expect(screen.getByTestId('ipr-refusal').textContent).toBe(t('ipRefresh.refuse.noService'))
    app.snapshot = snap({ wan: WAN })
    await flush()
    expect(screen.getByTestId('ipr-refusal').textContent).toBe(t('ipRefresh.refuse.notSupported'))
  })

  it('stresses the APN note after a run that found no match', async () => {
    app.snapshot = ready({
      ipRefresh: {
        active: false, phase: 'idle', method: null, tryN: 0, maxTries: 0, target: null, fromRange: null, currentRange: null,
        secondsInPhase: 0, secondsLeft: null, dataOnFailures: 0, stopping: false, resumed: false,
        last: {
          outcome: 'notMatched', tries: 5, target: '100', from: '10.45', to: '10.45',
          method: 'dataSwitch', refusal: null, bandsChanged: false, bandsBefore: null,
        },
        journalBlocked: false, cyclesLeft: 10,
      },
    })
    render(IpRefreshDialog)
    openIpRefresh()
    await flush()
    expect(screen.getByTestId('ipr-work-note').classList.contains('strong')).toBe(true)
  })

  it('describes the data switch and its estimate for a driver that uses it', async () => {
    render(IpRefreshDialog)
    openIpRefresh()
    await flush()
    expect(screen.getByTestId('ipr-method').textContent).toBe(t('ipRefresh.dialog.body'))
    expect(screen.getByTestId('ipr-estimate').textContent).toBe(t('ipRefresh.dialog.aboutSeconds', { n: '⁦35⁩' }))
  })

  it('describes the network search and estimates its tries by minutes', async () => {
    const s = ready()
    s.capabilities = { ...s.capabilities!, dataSwitch: true, ipMethods: ['networkSearch'] }
    app.snapshot = s
    render(IpRefreshDialog)
    openIpRefresh()
    await flush()
    expect(screen.getByTestId('ipr-method').textContent).toBe(t('ipRefresh.dialog.bodySearch'))
    // One search: up to about 3 minutes.
    expect(screen.getByTestId('ipr-estimate').textContent).toBe(t('ipRefresh.dialog.aboutMinutes', { n: '⁦3⁩' }))
    // Three searches with 20 s between them: 580 s, so up to 10 minutes.
    await fireEvent.click(screen.getByTestId('ipr-aim'))
    expect(screen.getByTestId('ipr-tries').textContent).toBe('3')
    expect(screen.getByTestId('ipr-estimate').textContent).toBe(t('ipRefresh.dialog.aboutMinutes', { n: '⁦10⁩' }))
    await fireEvent.click(screen.getByTestId('ipr-start'))
    expect(api.ipRefreshStart).toHaveBeenCalledWith('100.70', 3)
  })

  it('keeps the address start and its tail in address order in Arabic', async () => {
    setLanguage('ar')
    try {
      render(IpRefreshDialog)
      openIpRefresh('10.45')
      await flush()
      const input = screen.getByTestId('ipr-target')
      const row = input.parentElement as HTMLElement
      expect(row.getAttribute('dir')).toBe('ltr')
      expect(row.firstElementChild).toBe(input)
      expect(row.lastElementChild?.textContent).toBe('.x.x')
    } finally {
      setLanguage('en')
    }
  })
})

describe('Refresh IP rules', () => {
  it('accepts one or two numbers only', () => {
    expect(normalizeTarget('10')).toBe('10')
    expect(normalizeTarget(' 10.45 ')).toBe('10.45')
    expect(normalizeTarget('10.')).toBe('10')
    expect(normalizeTarget('010.045')).toBe('10.45')
    // Typed on an Arabic keyboard.
    expect(normalizeTarget('١٠٫٤٥')).toBe('10.45')
    expect(normalizeTarget('۱۰.۴۵')).toBe('10.45')
    for (const bad of ['', '10.45.1', '256', '1e2', '-1', '.10', '10..']) expect(normalizeTarget(bad), bad).toBeNull()
  })

  it('estimates longer runs for more tries', () => {
    expect(estimateSeconds(1)).toBe(35)
    expect(estimateSeconds(5)).toBeGreaterThan(estimateSeconds(2))
    expect(estimateSeconds(9)).toBe(estimateSeconds(5))
  })
})
