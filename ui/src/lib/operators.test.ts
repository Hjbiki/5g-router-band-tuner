import { describe, expect, it } from 'vitest'
import { identifyOperator } from './operators'

const logos = { zain: 'zain.svg', stc: 'stc.svg', mobily: 'mobily.png', salam: 'salam.svg' }
const id = (o: Parameters<typeof identifyOperator>[0]) => identifyOperator(o, logos)

describe('identifyOperator', () => {
  it('matches the three networks by PLMN, keeping the name the router reports', () => {
    expect(id({ name: 'Zain KSA', mccMnc: '42004' })).toEqual({ name: 'Zain KSA', logo: 'zain.svg', tile: 'dark', via: null })
    expect(id({ name: 'STC', mccMnc: '42001' })).toMatchObject({ name: 'STC', logo: 'stc.svg', tile: 'light' })
    expect(id({ name: 'Mobily', mccMnc: '42003' })).toMatchObject({ name: 'Mobily', logo: 'mobily.png', tile: 'light' })
  })

  it('leaves an unknown network with its own name and no logo', () => {
    expect(id({ name: 'Other Net', mccMnc: '00101' })).toEqual({ name: 'Other Net', logo: null, tile: 'light', via: null })
    expect(id({ name: '', mccMnc: '42004' }).logo).toBe('zain.svg')
  })

  it('an unverified MVNO on a host network gets its name, the neutral icon and no via', () => {
    // Even with a logo file present: a contributor's real SIM has to confirm it first.
    const salam = { name: 'Salam', logo: null, tile: 'light', via: null }
    expect(id({ name: 'Zain KSA', mccMnc: '42004', homePlmn: '42009' })).toEqual(salam)
    expect(id({ name: 'Zain KSA', mccMnc: '42004', spn: 'SALAM' })).toEqual(salam)
    expect(id({ name: 'Salam Mobile', mccMnc: '42004' })).toEqual(salam)
    expect(id({ name: 'STC', mccMnc: '42001', homePlmn: '42005' }).name).toBe('Virgin Mobile')
    expect(id({ name: 'Mobily', mccMnc: '42003', homePlmn: '42006' }).name).toBe('Lebara')
    expect(id({ name: 'Zain KSA', mccMnc: '42004', homePlmn: '42010' }).name).toBe('Red Bull Mobile')
  })

  it('reads Arabic SIM names and odd spacing', () => {
    expect(id({ name: 'Zain KSA', mccMnc: '42004', spn: 'سلام' }).name).toBe('Salam')
    expect(id({ name: 'Mobily', mccMnc: '42003', spn: 'Lebara KSA' }).name).toBe('Lebara')
    expect(id({ name: 'Zain KSA', mccMnc: '42004', spn: 'Red Bull' }).name).toBe('Red Bull Mobile')
  })

  it('trusts the SIM over the network, in that order', () => {
    expect(id({ name: 'Zain KSA', mccMnc: '42004', homePlmn: '42009', spn: 'Lebara' }).name).toBe('Salam')
    expect(id({ name: 'Zain KSA', mccMnc: '42004', spn: 'Lebara' }).name).toBe('Lebara')
  })

  it('shows a verified SIM on another known network with a via', () => {
    expect(id({ name: 'STC', mccMnc: '42001', homePlmn: '42004' })).toEqual({ name: 'Zain KSA', logo: 'zain.svg', tile: 'dark', via: 'stc' })
    // Its own network isn't a via.
    expect(id({ name: 'Zain KSA', mccMnc: '42004', homePlmn: '42004' }).via).toBeNull()
  })

  it('shows a SIM name no brand matches, and ignores an empty one', () => {
    expect(id({ name: 'Zain KSA', mccMnc: '00101', spn: 'New MVNO' })).toMatchObject({ name: 'Zain KSA', logo: 'zain.svg' })
    expect(id({ name: 'Net', mccMnc: '00101', spn: 'New MVNO' })).toEqual({ name: 'New MVNO', logo: null, tile: 'light', via: null })
    expect(id({ name: 'Net', mccMnc: '00101', spn: '  ' }).name).toBe('Net')
  })
})
