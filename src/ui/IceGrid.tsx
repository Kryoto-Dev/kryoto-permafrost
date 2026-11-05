import { useEffect, useRef } from 'react'

/**
 * The freeze, drawn in Kryoto's ASCII cells. Each cell is a piece of the game folder.
 * Cells that have been through the compressor are either solid, what the repack keeps,
 * or a faint dot, what it got rid of: the share of solid cells is the live ratio.
 */

const CELL_W = 9
const CELL_H = 15

function rand(a: number, b: number) {
  const n = Math.sin(a * 127.1 + b * 311.7 + 7.3) * 43758.5453
  return n - Math.floor(n)
}

export function IceGrid({ done, kept, className }: { done: number; kept: number; className?: string }) {
  const canvas = useRef<HTMLCanvasElement>(null)
  const state = useRef({ done, kept })
  state.current = { done, kept }

  useEffect(() => {
    const element = canvas.current
    const ctx = element?.getContext('2d')
    if (!element || !ctx) return
    let frame = 0
    let shown = 0
    const still = window.matchMedia('(prefers-reduced-motion: reduce)').matches

    const draw = (now: number) => {
      const { width, height } = element.getBoundingClientRect()
      const scale = window.devicePixelRatio || 1
      if (element.width !== Math.round(width * scale) || element.height !== Math.round(height * scale)) {
        element.width = Math.round(width * scale)
        element.height = Math.round(height * scale)
      }
      ctx.setTransform(scale, 0, 0, scale, 0, 0)
      ctx.clearRect(0, 0, width, height)

      const style = getComputedStyle(element)
      const ink = style.getPropertyValue('--foreground') || '#ededed'
      const cols = Math.max(1, Math.floor(width / CELL_W))
      const rows = Math.max(1, Math.floor(height / CELL_H))
      const count = cols * rows
      shown += (state.current.done - shown) * (still ? 1 : 0.08)
      const frontier = shown * count
      const keep = state.current.kept
      const time = now / 1000

      ctx.fillStyle = ink
      for (let i = 0; i < count; i++) {
        const col = i % cols
        const row = Math.floor(i / cols)
        const x = col * CELL_W
        const y = (rows - 1 - row) * CELL_H
        if (i > frontier + 3) {
          ctx.globalAlpha = 0.14
          ctx.fillRect(x + 3, y + 6, 2, 2)
        } else if (i > frontier - 3) {
          // At the edge the cells churn, the way the site's text scrambles and settles.
          const churn = rand(i, Math.floor(time * 12))
          ctx.globalAlpha = 0.2 + churn * 0.55
          if (churn > 0.5) ctx.fillRect(x, y, CELL_W - 1, (CELL_H - 1) / 2)
          else ctx.fillRect(x, y + (CELL_H - 1) / 2, CELL_W - 1, (CELL_H - 1) / 2)
        } else if (rand(col * 3.1, row * 7.7) < keep) {
          const glint = (Math.sin((col + row) * 0.22 - time * 1.6) + 1) / 2
          ctx.globalAlpha = 0.72 + glint * 0.28
          ctx.fillRect(x, y, CELL_W - 1, CELL_H - 1)
        } else {
          ctx.globalAlpha = 0.2
          ctx.fillRect(x, y, CELL_W - 1, CELL_H - 1)
        }
      }
      ctx.globalAlpha = 1
      if (!still) frame = requestAnimationFrame(draw)
    }
    frame = requestAnimationFrame(draw)
    const redraw = still ? window.setInterval(() => requestAnimationFrame(draw), 500) : 0
    return () => {
      cancelAnimationFrame(frame)
      window.clearInterval(redraw)
    }
  }, [])

  return <canvas ref={canvas} className={className} aria-hidden="true" />
}
