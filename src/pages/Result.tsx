import { useState } from 'react'
import { Button, Caption, Label, Stat } from '@/ui'
import { Copy, FolderOpen, Play } from '@/ui/pixel-icons'
import { api, duration, parseSize, ratio, size, type Report } from '@/lib/api'
import { cn } from '@/lib/utils'

/** One row of the walk from the original size down to the repack. */
function Step({ label, note, amount, left, total, strong }: { label: string; note?: string; amount: string; left: number; total: number; strong?: boolean }) {
  return (
    <li className="grid gap-1">
      <div className="flex items-baseline justify-between gap-4 text-xs">
        <span className={cn('min-w-0 truncate', strong ? 'font-bold text-foreground' : 'text-muted-foreground')}>
          {label}
          {note ? <span className="ml-2 text-[10px] text-muted-foreground">{note}</span> : null}
        </span>
        <span className={cn('shrink-0 tabular-nums', strong && 'font-bold')}>{amount}</span>
      </div>
      <div className="h-1.5 bg-secondary"><div className={cn('h-full', strong ? 'bg-primary' : 'bg-muted-foreground')} style={{ width: `${Math.max(0.5, (left / Math.max(1, total)) * 100)}%` }} /></div>
    </li>
  )
}

export function Result({ report, title, kryotoSize, onAgain }: { report: Report; title: string; kryotoSize: string | null; onAgain: () => void }) {
  const [copied, setCopied] = useState(false)
  const folder = report.outputs[0]?.replace(/[\\/][^\\/]*$/, '') ?? ''
  const saved = report.original - report.totalOutput
  const share = ratio(report.totalOutput, report.original)
  const times = report.totalOutput > 0 ? report.original / report.totalOutput : 0
  const kryoto = kryotoSize ? parseSize(kryotoSize) : null

  const copyInfo = async () => {
    await navigator.clipboard.writeText(await api.readText(report.info))
    setCopied(true)
    setTimeout(() => setCopied(false), 1400)
  }

  const afterRepeats = report.original - report.duplicateBytes
  const afterJpegs = afterRepeats - report.jpegSaved

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="min-h-0 flex-1 overflow-y-auto px-6 py-6">
        <div className="mx-auto grid w-full max-w-5xl gap-6">
          <header className="grid gap-4 md:grid-cols-[1fr_auto] md:items-end">
            <div className="grid gap-1">
              <Caption>Frozen{report.tested ? ', every byte checked' : ''}</Caption>
              <h1 className="truncate text-2xl font-bold" title={title}>{title}</h1>
            </div>
            <div className="flex items-baseline gap-3 tabular-nums">
              <span className="text-sm text-muted-foreground">{size(report.original)}</span>
              <span className="text-4xl font-bold">{size(report.totalOutput)}</span>
            </div>
          </header>

          <dl className="kryo-radius grid grid-cols-2 gap-4 border border-border p-4 sm:grid-cols-3 lg:grid-cols-6">
            <Stat label="Ratio" value={`${share.toFixed(1)}%`} />
            <Stat label="Smaller by" value={`${times.toFixed(2)}x`} />
            <Stat label="Saved" value={size(saved)} />
            <Stat label="Took" value={duration(report.seconds)} />
            <Stat label="Speed" value={`${size(report.original / Math.max(1, report.seconds))}/s`} />
            <Stat label="Memory to install" value={size(report.memoryToInstall)} />
          </dl>

          {kryoto ? (
            <p className="kryo-radius border border-border p-4 text-xs">
              {report.totalOutput < kryoto
                ? <>This repack is <strong>{size(kryoto - report.totalOutput)}</strong> smaller than the {size(kryoto)} Kryoto download ({ratio(report.totalOutput, kryoto).toFixed(1)}% of it).</>
                : <>The Kryoto download is {size(kryoto)}, so this repack saves nothing over it. A stronger mode may.</>}
            </p>
          ) : null}

          <div className="grid gap-6 lg:grid-cols-2">
            <section className="grid content-start gap-3">
              <Label>Where the size went</Label>
              <ol className="grid gap-3">
                <Step label="The game folder" amount={size(report.original)} left={report.original} total={report.original} />
                <Step label="Repeated data removed" note={report.duplicateBytes > 0 ? undefined : 'none found'} amount={`-${size(report.duplicateBytes)}`} left={afterRepeats} total={report.original} />
                {report.jpegs > 0 ? <Step label="JPEGs recompressed" note={`${report.jpegs} images`} amount={`-${size(report.jpegSaved)}`} left={afterJpegs} total={report.original} /> : null}
                <Step label="Compressed" note={report.unwrappedStreams > 0 ? `${report.unwrappedStreams} packed streams opened first` : undefined} amount={`-${size(Math.max(0, afterJpegs - report.packed))}`} left={report.packed} total={report.original} />
                {report.totalOutput > report.packed ? <Step label="Setup and art" amount={`+${size(report.totalOutput - report.packed)}`} left={report.totalOutput} total={report.original} /> : null}
                <Step label="The repack" amount={size(report.totalOutput)} left={report.totalOutput} total={report.original} strong />
              </ol>
            </section>

            <section className="grid content-start gap-3">
              <Label>By kind of file</Label>
              <table className="w-full text-xs">
                <thead className="text-[10px] uppercase tracking-wider text-muted-foreground">
                  <tr>
                    <th scope="col" className="pb-1 text-left font-normal">Kind</th>
                    <th scope="col" className="pb-1 text-right font-normal">Files</th>
                    <th scope="col" className="pb-1 text-right font-normal">Before</th>
                    <th scope="col" className="pb-1 text-right font-normal">After</th>
                    <th scope="col" className="pb-1 text-right font-normal">Ratio</th>
                  </tr>
                </thead>
                <tbody>
                  {report.classes.map((row) => (
                    <tr key={row.label} className="border-t border-border">
                      <td className="py-1.5">{row.label}</td>
                      <td className="py-1.5 text-right tabular-nums text-muted-foreground">{row.files}</td>
                      <td className="py-1.5 text-right tabular-nums text-muted-foreground">{size(row.original)}</td>
                      <td className="py-1.5 text-right tabular-nums">{size(row.packed)}</td>
                      <td className="py-1.5 text-right tabular-nums">{ratio(row.packed, row.original).toFixed(1)}%</td>
                    </tr>
                  ))}
                </tbody>
              </table>
              <Caption>Files {report.files} · Blocks {report.codecs.reduce((n, c) => n + c.blocks, 0)} · {report.codecs.map((c) => c.codec).join(', ')}</Caption>
            </section>
          </div>

          <section className="grid gap-2">
            <Label>Files</Label>
            <ul className="grid gap-1 text-xs text-muted-foreground">
              {report.outputs.map((path) => <li key={path} className="truncate" title={path}>{path}</li>)}
            </ul>
          </section>
        </div>
      </div>

      <footer className="flex shrink-0 flex-wrap items-center gap-2 border-t border-border px-6 py-3">
        <Button variant="ghost" onClick={onAgain}>New repack</Button>
        <span className="flex-1" />
        <Button onClick={() => void copyInfo()}>
          <Copy className="size-3.5" aria-hidden="true" />
          {copied ? 'Copied' : 'Copy info'}
        </Button>
        <Button onClick={() => void api.openFolder(folder)}>
          <FolderOpen className="size-3.5" aria-hidden="true" />
          Open folder
        </Button>
        {report.setup ? (
          <Button variant="primary" onClick={() => void api.runSetup(report.setup!)}>
            <Play className="size-3.5" aria-hidden="true" />
            Try the setup
          </Button>
        ) : null}
      </footer>
    </div>
  )
}
