import { useState } from 'react'
import { Button, Caption, Label } from '@/ui'
import { FolderOpen } from '@/ui/pixel-icons'
import { api, duration, ratio, size, type LogEntry } from '@/lib/api'
import { LEVELS } from '@/lib/levels'
import { cn } from '@/lib/utils'
import wordmark from '../../art/permafrost-wordmark.svg'

export function Start({ log, hovering, busy, error, onPick }: {
  log: LogEntry[]
  hovering: boolean
  busy: boolean
  error: string | null
  onPick: (folder: string) => void
}) {
  const [picking, setPicking] = useState(false)
  const browse = async () => {
    setPicking(true)
    const folder = await api.pickFolder('Pick the game folder').catch(() => null)
    setPicking(false)
    if (folder) onPick(folder)
  }
  const saved = log.reduce((sum, e) => sum + Math.max(0, e.original - e.output), 0)

  return (
    <div className="mx-auto grid w-full max-w-4xl gap-8 px-8 py-10">
      <div
        className={cn(
          'kryo-radius grid place-items-center gap-6 border border-dashed px-8 py-14 text-center transition-colors',
          hovering ? 'border-foreground bg-secondary' : 'border-border',
        )}
      >
        <img src={wordmark} alt="Permafrost" className="h-14 max-w-full" />
        <p className="max-w-md text-sm text-muted-foreground">
          Drop a game folder here. It comes out as one small setup that thaws the game back, bit for bit.
        </p>
        <Button variant="primary" size="lg" disabled={busy || picking} onClick={() => void browse()}>
          <FolderOpen className="size-4" aria-hidden="true" />
          {busy ? 'Reading the folder' : 'Pick a folder'}
        </Button>
        {error ? <p role="alert" className="text-xs text-destructive">{error}</p> : null}
      </div>

      {log.length > 0 ? (
        <section className="grid gap-3">
          <div className="flex items-baseline justify-between">
            <Label>Past repacks</Label>
            <Caption>{size(saved)} saved so far</Caption>
          </div>
          <div className="kryo-radius overflow-hidden border border-border">
            <table className="w-full text-left text-xs">
              <thead className="text-[10px] uppercase tracking-wider text-muted-foreground">
                <tr className="border-b border-border">
                  <th scope="col" className="px-4 py-2 font-normal">Game</th>
                  <th scope="col" className="px-4 py-2 font-normal">Mode</th>
                  <th scope="col" className="px-4 py-2 text-right font-normal">Original</th>
                  <th scope="col" className="px-4 py-2 text-right font-normal">Repack</th>
                  <th scope="col" className="px-4 py-2 text-right font-normal">Ratio</th>
                  <th scope="col" className="px-4 py-2 text-right font-normal">Took</th>
                </tr>
              </thead>
              <tbody>
                {log.slice(0, 8).map((entry) => (
                  <tr key={entry.date + entry.title} className="border-b border-border last:border-0">
                    <td className="max-w-56 truncate px-4 py-2">
                      <button type="button" className="hover:underline" title="Open the folder" onClick={() => void api.openFolder(entry.folder)}>
                        {entry.title}
                      </button>
                    </td>
                    <td className="px-4 py-2 text-muted-foreground">{LEVELS.find((l) => l.value === entry.level)?.label}</td>
                    <td className="px-4 py-2 text-right tabular-nums text-muted-foreground">{size(entry.original)}</td>
                    <td className="px-4 py-2 text-right tabular-nums">{size(entry.output)}</td>
                    <td className="px-4 py-2 text-right tabular-nums">{ratio(entry.output, entry.original).toFixed(1)}%</td>
                    <td className="px-4 py-2 text-right tabular-nums text-muted-foreground">{duration(entry.seconds)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </section>
      ) : null}
    </div>
  )
}
