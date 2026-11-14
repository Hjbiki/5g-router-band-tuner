import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/svelte'
import { tick } from 'svelte'
import InfoTip from './InfoTip.svelte'

const LABEL = 'What is SINR?'
const TEXT = 'How clean the signal is.'

function tip(): HTMLElement | null {
  return screen.queryByRole('note')
}

function button(): HTMLElement {
  return screen.getByRole('button', { name: LABEL })
}

async function wait(ms: number) {
  await vi.advanceTimersByTimeAsync(ms)
  await tick()
}

/** The pointer rests on the button until the tip is open. */
async function hoverOpen() {
  await fireEvent.mouseEnter(button())
  await wait(400)
}

describe('InfoTip', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    render(InfoTip, { label: LABEL, text: TEXT })
  })
  afterEach(() => {
    cleanup()
    vi.useRealTimers()
  })

  it('opens when the pointer rests on it and closes after the pointer leaves', async () => {
    await hoverOpen()
    expect(tip()?.textContent).toBe(TEXT)
    expect(button().getAttribute('aria-expanded')).toBe('true')
    await fireEvent.mouseLeave(button())
    await wait(600)
    expect(tip()).toBeNull()
  })

  it('stays closed when the pointer only passes over it', async () => {
    await fireEvent.mouseEnter(button())
    await wait(40)
    await fireEvent.mouseLeave(button())
    await wait(600)
    expect(tip()).toBeNull()
  })

  it('stays open while the pointer moves from the button into the tip', async () => {
    await hoverOpen()
    await fireEvent.mouseLeave(button())
    await fireEvent.mouseEnter(tip()!)
    await wait(600)
    expect(tip()).not.toBeNull()
    await fireEvent.mouseLeave(tip()!)
    await wait(600)
    expect(tip()).toBeNull()
  })

  it('a click keeps open a tip that the pointer opened, and a second click closes it', async () => {
    await hoverOpen()
    await fireEvent.click(button())
    expect(tip()).not.toBeNull()
    await fireEvent.mouseLeave(button())
    await wait(600)
    expect(tip()).not.toBeNull()
    await fireEvent.click(button())
    expect(tip()).toBeNull()
  })

  it('still opens and closes by clicking alone', async () => {
    await fireEvent.click(button())
    expect(tip()?.textContent).toBe(TEXT)
    await wait(600)
    expect(tip()).not.toBeNull()
    await fireEvent.click(button())
    expect(tip()).toBeNull()
  })

  const closers: [string, () => Promise<unknown>][] = [
    ['Esc', () => fireEvent.keyDown(window, { key: 'Escape' })],
    ['a click elsewhere', () => fireEvent.click(document.body)],
    ['scrolling', () => fireEvent.scroll(window)],
    ['resizing the window', () => fireEvent(window, new Event('resize'))],
  ]
  for (const [name, close] of closers) {
    it(`${name} closes it, whether a click or the pointer opened it`, async () => {
      await fireEvent.click(button())
      expect(tip()).not.toBeNull()
      await close()
      await tick()
      expect(tip()).toBeNull()

      await hoverOpen()
      expect(tip()).not.toBeNull()
      await close()
      await tick()
      expect(tip()).toBeNull()
      // And it doesn't come back by itself afterwards.
      await wait(600)
      expect(tip()).toBeNull()
    })
  }
})
