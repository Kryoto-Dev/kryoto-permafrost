// Renders Permafrost's art from code: `pnpm brand`.
//
// The K// comes from src/ui/ascii/mark.ts, traced from kryo.to's own image, and the
// lettering from src/ui/ascii/cells.ts, the same cells the site's logo is drawn with.
// Same code in, same pixels out.
//
//   art/permafrost-wordmark.svg      the name, in Kryoto's block lettering
//   art/permafrost-banner.png        README banner
//   art/permafrost-icon.png          the app's icon, large
//   art/kryo-file.png                what a .kryo looks like
//   art/default-cover.png            cover art for a repack that brings none
//   src-tauri/icons/icon.ico         the app
//   crates/setup/setup.ico           the setup, before a game's icon replaces it
//   docs/logo.png                    the README mark

import sharp from 'sharp'
import { mkdirSync, writeFileSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { asciiPaths, gridSize } from '../src/ui/ascii/cells.ts'
import { MARK_H, MARK_PATH, MARK_W } from '../src/ui/ascii/mark.ts'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const at = (...p) => path.join(root, ...p)
for (const dir of ['art', 'docs', 'src-tauri/icons']) mkdirSync(at(dir), { recursive: true })

const INK = '#ededed'
const TILE = '#0a0a0a'
const ICE = '#9fd3ee'
const DEEP = '#16222b'

/** The site's lettering has no P, E, M, A, F or S. These follow the same six rows. */
const LETTERS = {
  P: ['██████╗ ', '██╔══██╗', '██████╔╝', '██╔═══╝ ', '██║     ', '╚═╝     '],
  E: ['███████╗', '██╔════╝', '█████╗  ', '██╔══╝  ', '███████╗', '╚══════╝'],
  R: ['██████╗ ', '██╔══██╗', '██████╔╝', '██╔══██╗', '██║  ██║', '╚═╝  ╚═╝'],
  M: ['███╗   ███╗', '████╗ ████║', '██╔████╔██║', '██║╚██╔╝██║', '██║ ╚═╝ ██║', '╚═╝     ╚═╝'],
  A: [' █████╗ ', '██╔══██╗', '███████║', '██╔══██║', '██║  ██║', '╚═╝  ╚═╝'],
  F: ['███████╗', '██╔════╝', '█████╗  ', '██╔══╝  ', '██║     ', '╚═╝     '],
  O: [' ██████╗ ', '██╔═══██╗', '██║   ██║', '██║   ██║', '╚██████╔╝', ' ╚═════╝ '],
  S: ['███████╗', '██╔════╝', '███████╗', '╚════██║', '███████║', '╚══════╝'],
  T: ['████████╗', '╚══██╔══╝', '   ██║   ', '   ██║   ', '   ██║   ', '   ╚═╝   '],
}

const lettering = (text) => {
  const rows = ['', '', '', '', '', '']
  for (const ch of text) LETTERS[ch].forEach((line, i) => (rows[i] += line))
  return rows
}

const WORD = lettering('PERMAFROST')

function wordGroup(color) {
  return asciiPaths(WORD)
    .map((p) => `<path d="${p.d}" fill="${color}"${p.alpha < 1 ? ` fill-opacity="${p.alpha}"` : ''}/>`)
    .join('')
}

function rand(a, b) {
  const n = Math.sin(a * 127.1 + b * 311.7 + 7.3) * 43758.5453
  return n - Math.floor(n)
}

/** A field of ice cells, heavier towards the bottom. */
function ice(width, height, cell, { from = 0, color = ICE, seed = 1 } = {}) {
  const cw = cell
  const ch = cell * (5 / 3)
  let out = ''
  for (let y = 0; y * ch < height; y++) {
    const depth = y / Math.max(1, height / ch - 1)
    if (depth < from) continue
    const strength = (depth - from) / Math.max(0.001, 1 - from)
    for (let x = 0; x * cw < width; x++) {
      const r = rand(x + seed, y)
      if (r > strength * 1.05) continue
      const alpha = [0.08, 0.16, 0.3, 0.5][Math.min(3, Math.floor(rand(x, y + seed) * 4 * (0.4 + strength)))]
      out += `<rect x="${(x * cw).toFixed(2)}" y="${(y * ch).toFixed(2)}" width="${cw}" height="${ch.toFixed(2)}" fill="${color}" fill-opacity="${alpha}"/>`
    }
  }
  return out
}

function markGroup(x, y, width, color, id) {
  const k = width / MARK_W
  return `<path id="${id ?? 'mark'}" transform="translate(${x} ${y}) scale(${k})" d="${MARK_PATH}" fill="${color}"/>`
}

const svg = (w, h, body) => `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}" viewBox="0 0 ${w} ${h}">${body}</svg>`

/** The K// locked in a block of ice, its lower half still under the frost. */
function iconSvg(size, { frame = true } = {}) {
  const S = 1024
  const small = size < 48
  const r = S * 0.2
  const w = small ? S * 0.82 : S * 0.7
  const h = (w * MARK_H) / MARK_W
  const x = (S - w) / 2
  const y = (S - h) / 2
  const pad = S * 0.07
  const line = S * 0.012
  const box = frame && !small
    ? `<rect x="${pad}" y="${pad}" width="${S - pad * 2}" height="${S - pad * 2}" rx="${r * 0.55}" fill="none" stroke="${ICE}" stroke-opacity=".55" stroke-width="${line}"/>
       <rect x="${pad + line * 3}" y="${pad + line * 3}" width="${S - (pad + line * 3) * 2}" height="${S - (pad + line * 3) * 2}" rx="${r * 0.42}" fill="none" stroke="${ICE}" stroke-opacity=".3" stroke-width="${line}"/>`
    : ''
  const frost = small ? '' : `<clipPath id="tile"><rect width="${S}" height="${S}" rx="${r}"/></clipPath><g clip-path="url(#tile)">${ice(S, S, 32, { from: 0.45 })}</g>`
  return svg(S, S, `<rect width="${S}" height="${S}" rx="${r}" fill="${TILE}"/>${frost}${box}${markGroup(x, y, w, INK)}`)
}

const png = (markup, width, height) => sharp(Buffer.from(markup), { density: 96 }).resize(width, height).png({ compressionLevel: 9 }).toBuffer()

async function ico(file, make) {
  const sizes = [16, 24, 32, 48, 64, 128, 256]
  const images = await Promise.all(sizes.map((s) => png(make(s), s, s)))
  const head = Buffer.alloc(6 + sizes.length * 16)
  head.writeUInt16LE(1, 2)
  head.writeUInt16LE(sizes.length, 4)
  let offset = head.length
  sizes.forEach((s, i) => {
    const row = 6 + i * 16
    head.writeUInt8(s % 256, row)
    head.writeUInt8(s % 256, row + 1)
    head.writeUInt16LE(1, row + 4)
    head.writeUInt16LE(32, row + 6)
    head.writeUInt32LE(images[i].length, row + 8)
    head.writeUInt32LE(offset, row + 12)
    offset += images[i].length
  })
  writeFileSync(file, Buffer.concat([head, ...images]))
}

// The name.
const grid = gridSize(WORD)
writeFileSync(at('art/permafrost-wordmark.svg'), svg(grid.width, grid.height, wordGroup(INK)))

// Icons.
await ico(at('src-tauri/icons/icon.ico'), (s) => iconSvg(s))
await ico(at('crates/setup/setup.ico'), (s) => iconSvg(s, { frame: false }))
writeFileSync(at('art/permafrost-icon.png'), await png(iconSvg(1024), 1024, 1024))
writeFileSync(at('docs/logo.png'), await png(iconSvg(256), 256, 256))

// The README banner: the name above the frost line, the K// sunk in the ice under it.
{
  const W = 1600
  const H = 480
  const scale = (W * 0.62) / grid.width
  const x = (W - grid.width * scale) / 2
  const body = `<rect width="${W}" height="${H}" fill="${TILE}"/>
    ${ice(W, H, 14, { from: 0.42 })}
    <g transform="translate(${x} ${H * 0.2}) scale(${scale})">${wordGroup(INK)}</g>
    ${markGroup(W / 2 - 62, H * 0.66, 124, DEEP)}`
  writeFileSync(at('art/permafrost-banner.png'), await png(svg(W, H, body), W, H))
}

// A .kryo: a folded page in double lines, frozen from the bottom up.
{
  const S = 512
  const fold = 112
  const left = 84
  const right = S - 84
  const top = 32
  const bottom = S - 32
  const page = `M${left} ${top}H${right - fold}L${right} ${top + fold}V${bottom}H${left}Z`
  const body = `<clipPath id="page"><path d="${page}"/></clipPath>
    <path d="${page}" fill="${TILE}"/>
    <g clip-path="url(#page)">${ice(S, S, 16, { from: 0.35 })}</g>
    <path d="${page}" fill="none" stroke="${INK}" stroke-width="10"/>
    <path d="M${right - fold} ${top}V${top + fold}H${right}" fill="none" stroke="${INK}" stroke-width="10"/>
    ${markGroup(left + 40, 232, right - left - 80, INK)}`
  writeFileSync(at('art/kryo-file.png'), await png(svg(S, S, body), S, S))
}

// The cover a repack gets when it brings none of its own: a game case going under.
{
  const W = 744
  const H = 1200
  const body = `<rect width="${W}" height="${H}" fill="#0c0e10"/>
    ${ice(W, H, 24, { from: 0.3, seed: 7 })}
    <rect x="96" y="210" width="552" height="780" fill="none" stroke="${INK}" stroke-width="6"/>
    <rect x="114" y="228" width="516" height="744" fill="none" stroke="${INK}" stroke-opacity=".5" stroke-width="6"/>
    ${markGroup(172, 470, 400, INK)}`
  writeFileSync(at('art/default-cover.png'), await png(svg(W, H, body), W, H))
}

console.log('art rendered')
