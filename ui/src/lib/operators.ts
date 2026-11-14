// Who the SIM belongs to, and the logo to show. The registered network (PLMN) alone isn't enough:
// an MVNO's SIM can register on its host, so a Salam SIM could look like Zain.
//
// A brand gets a logo and "via <host>" only once it is `verified`: a real SIM has confirmed its
// codes through a diagnostics export. Until then it shows its name with the neutral icon, because
// the public lists disagree.
//
// Logos live in ui/src/assets/operators as `<id>.svg`, `.png` or `.webp`, and are third-party
// trademarks: see the README in that folder.

type Brand = {
  id: string
  name: string
  /** PLMN codes this brand's SIMs carry, or the network it runs. */
  plmn: string[]
  /** How the SIM's name or the router's network name spells it, lower case with no spaces. */
  aliases: string[]
  verified: boolean
  /** Backing for the logo, the same in both themes: Zain's light grey needs dark. */
  tile?: 'light' | 'dark'
}

const BRANDS: Brand[] = [
  { id: 'stc', name: 'stc', plmn: ['42001'], aliases: ['stc', 'aljawal', 'الاتصالاتالسعودية'], verified: true, tile: 'light' },
  { id: 'mobily', name: 'Mobily', plmn: ['42003'], aliases: ['mobily', 'موبايلي'], verified: true, tile: 'light' },
  { id: 'zain', name: 'Zain KSA', plmn: ['42004'], aliases: ['zain', 'زين'], verified: true, tile: 'dark' },
  { id: 'virgin', name: 'Virgin Mobile', plmn: ['42005'], aliases: ['virgin', 'فيرجن'], verified: false },
  { id: 'lebara', name: 'Lebara', plmn: ['42006'], aliases: ['lebara', 'ليبارا'], verified: false },
  { id: 'salam', name: 'Salam', plmn: ['42009'], aliases: ['salam', 'سلام'], verified: false },
  { id: 'redbull', name: 'Red Bull Mobile', plmn: ['42010'], aliases: ['redbull', 'ريدبول'], verified: false },
]

const FILES = import.meta.glob('../assets/operators/*.{svg,png,webp}', {
  eager: true,
  query: '?url',
  import: 'default',
}) as Record<string, string>

function bundledLogos(files: Record<string, string>): Record<string, string> {
  const out: Record<string, string> = {}
  for (const [path, url] of Object.entries(files)) {
    const id = path.split('/').pop()?.split('.')[0]
    if (id) out[id] = url
  }
  return out
}
const LOGOS = bundledLogos(FILES)

const byPlmn = (code: string | null | undefined) => (code ? BRANDS.find((b) => b.plmn.includes(code)) : undefined)

function byName(text: string | null | undefined): Brand | undefined {
  if (!text) return undefined
  const plain = text.toLowerCase().replace(/[\s\-_.]/g, '')
  return BRANDS.find((b) => b.aliases.some((a) => plain.includes(a)))
}

export type Identity = {
  name: string
  logo: string | null
  tile: 'light' | 'dark'
  /** The network actually carrying the SIM, when that's a different, known operator. */
  via: string | null
}

export type OperatorInput = {
  /** The name the router reports for the registered network. */
  name: string
  /** The registered network. */
  mccMnc: string
  /** The name stored on the SIM, when the router reports one. */
  spn?: string | null
  /** The SIM's own network: the first digits of its IMSI, and nothing more of it. */
  homePlmn?: string | null
}

/**
 * Most trustworthy first: the SIM's home network, the SIM's name, the name the router reports,
 * then the registered network. A SIM name no brand matches is still shown, since it's the SIM's
 * own. Only verified brands get a logo or a "via".
 */
export function identifyOperator(op: OperatorInput, logos: Record<string, string> = LOGOS): Identity {
  const brand = byPlmn(op.homePlmn) ?? byName(op.spn) ?? byName(op.name) ?? byPlmn(op.mccMnc)
  const spn = op.spn?.trim()
  if (!brand) return { name: spn || op.name, logo: null, tile: 'light', via: null }
  if (!brand.verified) return { name: brand.name, logo: null, tile: 'light', via: null }
  const carrier = byPlmn(op.mccMnc)
  return {
    // A verified host's name as the router spells it, e.g. "Zain KSA".
    name: carrier === brand && op.name ? op.name : brand.name,
    logo: logos[brand.id] ?? null,
    tile: brand.tile ?? 'light',
    via: carrier && carrier !== brand ? carrier.name : null,
  }
}
