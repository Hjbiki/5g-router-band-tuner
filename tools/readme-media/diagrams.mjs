// Writes the safe-apply diagram for each language and the README's language and Download
// buttons, as SVG.
import { writeFileSync } from 'node:fs'
import { join } from 'node:path'

const FONT = "-apple-system,'Segoe UI','Noto Sans','Noto Sans Arabic',Tahoma,Arial,sans-serif"
const STYLE = `<style>
  text { font-family: ${FONT}; fill: #1f2328; }
  .sub { fill: #59636e; font-size: 12px; }
  .box { fill: #f6f8fa; stroke: #8c959f; stroke-width: 1.2; }
  .good { fill: #dafbe1; stroke: #1a7f37; stroke-width: 1.2; }
  .bad { fill: #ffebe9; stroke: #cf222e; stroke-width: 1.2; }
  .warn { fill: #fff8c5; stroke: #9a6700; stroke-width: 1.2; }
  .line { stroke: #59636e; stroke-width: 1.5; fill: none; }
  .redline { stroke: #cf222e; stroke-width: 1.5; fill: none; stroke-dasharray: 5 4; }
  .head { fill: #59636e; }
  .redhead { fill: #cf222e; }
  .redtext { fill: #cf222e; font-size: 13px; }
  @media (prefers-color-scheme: dark) {
    text { fill: #f0f6fc; }
    .sub { fill: #9198a1; }
    .box { fill: #21262d; stroke: #6e7681; }
    .good { fill: #12361f; stroke: #3fb950; }
    .bad { fill: #3d1416; stroke: #f85149; }
    .warn { fill: #3a2c07; stroke: #d29922; }
    .line { stroke: #9198a1; }
    .redline { stroke: #f85149; }
    .head { fill: #9198a1; }
    .redhead { fill: #f85149; }
    .redtext { fill: #f85149; }
  }
</style>`

const LABELS = {
  en: {
    steps: [['Saved', 'current bands'], ['Applied', 'new bands sent'], ['Checked', 'read back'], ['Reconnected', 'up to 90 s'], ['Trial', '3 min'], ['Kept', 'done']],
    lost: 'Internet lost', back: 'Previous bands come back', backSub: 'by themselves', auto: 'If that fails: Auto', autoSub: 'last resort',
  },
  ar: {
    steps: [['الحفظ', 'النطاقات الحالية'], ['التطبيق', 'إرسال النطاقات'], ['التحقق', 'قراءة الإعداد'], ['أُعيد الاتصال', 'حتى 90 ثانية'], ['التجربة', '3 دقائق'], ['الإبقاء', 'تم']],
    lost: 'انقطع الإنترنت', back: 'ترجع النطاقات السابقة', backSub: 'تلقائيًا', auto: 'إذا تعذر: الوضع التلقائي', autoSub: 'الملاذ الأخير',
  },
}

const W = 900
const BW = 124
const BH = 62
const GAP = 27.2
const Y = 20

function safeApply(lang) {
  const rtl = lang === 'ar'
  const L = LABELS[lang]
  const px = (x) => (rtl ? W - x : x)
  const n = (v) => v.toFixed(1)
  const out = [`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${W} 250" width="${W}" role="img" direction="${rtl ? 'rtl' : 'ltr'}">`, STYLE]
  const rect = (x, y, w, h, cls) => out.push(`<rect x="${n(rtl ? px(x + w) : x)}" y="${y}" width="${w}" height="${h}" rx="8" class="${cls}"/>`)
  const text = (cx, y, s, { cls, fs, weight } = {}) =>
    out.push(`<text x="${n(px(cx))}" y="${y}" text-anchor="middle"${cls ? ` class="${cls}"` : ''}${fs ? ` font-size="${fs}"` : ''}${weight ? ` font-weight="${weight}"` : ''}>${s}</text>`)
  const line = (x1, y1, x2, y2, cls) => out.push(`<line x1="${n(px(x1))}" y1="${y1}" x2="${n(px(x2))}" y2="${y2}" class="${cls}"/>`)
  const arrow = (x1, y1, x2, y2, red = false) => {
    line(x1, y1, x2, y2, red ? 'redline' : 'line')
    const X = px(x2)
    const head = red ? 'redhead' : 'head'
    if (y1 === y2) {
      const s = X > px(x1) ? 1 : -1
      out.push(`<path d="M${n(X)} ${y2} l${-7 * s} -4 v8 z" class="${head}"/>`)
    } else {
      const s = y2 > y1 ? 1 : -1
      out.push(`<path d="M${n(X)} ${y2} l-4 ${-7 * s} h8 z" class="${head}"/>`)
    }
  }

  const xs = L.steps.map((_, i) => 10 + i * (BW + GAP))
  L.steps.forEach(([title, sub], i) => {
    rect(xs[i], Y, BW, BH, i === 5 ? 'good' : i === 3 || i === 4 ? 'warn' : 'box')
    text(xs[i] + BW / 2, Y + 27, title, { fs: 15, weight: 600 })
    text(xs[i] + BW / 2, Y + 47, sub, { cls: 'sub' })
    if (i < 5) arrow(xs[i] + BW + 2, Y + BH / 2, xs[i + 1] - 2, Y + BH / 2)
  })

  // Losing service while reconnecting or on trial leads down to the revert.
  const mid3 = xs[3] + BW / 2
  const mid4 = xs[4] + BW / 2
  const bar = 124
  const by = 165
  line(mid3, Y + BH + 2, mid3, bar, 'redline')
  line(mid4, Y + BH + 2, mid4, bar, 'redline')
  line(mid3, bar, mid4, bar, 'redline')
  const cx = (mid3 + mid4) / 2
  text(cx + 62, 147, L.lost, { cls: 'redtext' })
  arrow(cx, bar, cx, by - 3, true)
  const bw = 250
  rect(cx - bw / 2, by, bw, BH, 'bad')
  text(cx, by + 27, L.back, { fs: 15, weight: 600 })
  text(cx, by + 47, L.backSub, { cls: 'sub' })
  const ax = xs[0] + 150
  rect(ax, by, bw, BH, 'box')
  text(ax + bw / 2, by + 27, L.auto, { fs: 15, weight: 600 })
  text(ax + bw / 2, by + 47, L.autoSub, { cls: 'sub' })
  arrow(cx - bw / 2 - 2, by + BH / 2, ax + bw + 2, by + BH / 2, true)
  out.push('</svg>')
  return out.join('\n') + '\n'
}

function button(label, on, lang) {
  const width = 92
  const [fill, fg, stroke] = on ? ['#1f6feb', '#ffffff', '#1f6feb'] : ['#00000000', '#8b949e', '#8b949e']
  const dir = lang === 'ar' ? ' direction="rtl"' : ''
  return `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="34" viewBox="0 0 ${width} 34" role="img" aria-label="${label}"${dir}>` +
    `<rect x="0.5" y="0.5" width="${width - 1}" height="33" rx="7" fill="${fill}" stroke="${stroke}"/>` +
    `<text x="${width / 2}" y="22.5" text-anchor="middle" font-family="${FONT}" font-size="15" font-weight="${on ? 700 : 400}" fill="${fg}">${label}</text></svg>\n`
}

/** The README's Download button: an arrow into a tray at the reading start, then the label. */
function downloadButton(label, lang, width) {
  const rtl = lang === 'ar'
  const icon = rtl ? width - 38 : 38
  const mid = rtl ? (width - 62) / 2 : 62 + (width - 62) / 2
  const dir = rtl ? ' direction="rtl"' : ''
  return `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="52" viewBox="0 0 ${width} 52" role="img" aria-label="${label}"${dir}>` +
    `<rect x="0.5" y="0.5" width="${width - 1}" height="51" rx="10" fill="#1f6feb" stroke="#1f6feb"/>` +
    `<path d="M${icon} 15v15m-6 -6l6 6l6 -6M${icon - 9} 37h18" fill="none" stroke="#ffffff" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"/>` +
    `<text x="${mid}" y="32.5" text-anchor="middle" font-family="${FONT}" font-size="18" font-weight="700" fill="#ffffff">${label}</text></svg>\n`
}

export function writeDiagrams(mediaDir, langs) {
  for (const lang of langs) writeFileSync(join(mediaDir, lang, 'safe-apply.svg'), safeApply(lang))
  for (const [lang, label] of [['en', 'English'], ['ar', 'العربية']]) {
    writeFileSync(join(mediaDir, `lang-${lang}-on.svg`), button(label, true, lang))
    writeFileSync(join(mediaDir, `lang-${lang}-off.svg`), button(label, false, lang))
  }
  // Widths fit each label at 18 px bold (about 200 and 100 px) with the icon and even margins.
  for (const [lang, label, width] of [['en', 'Download for Windows', 300], ['ar', 'تنزيل لويندوز', 220]]) {
    writeFileSync(join(mediaDir, `download-${lang}.svg`), downloadButton(label, lang, width))
  }
}
