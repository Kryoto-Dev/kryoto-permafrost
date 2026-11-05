import { useEffect, useRef, useState } from 'react'
import { AsciiBar, AsciiSpark, Button, Caption, Label, Stat } from '@/ui'
import { Check as CheckIcon } from '@/ui/pixel-icons'
import { duration, ratio, size, type Status } from '@/lib/api'
import { IceGrid } from '@/ui/IceGrid'
import { api } from '@/lib/api'
import { cn } from '@/lib/utils'

const STAGES = [
  { id: 'reading', label: 'Reading and finding repeats' },
  { id: 'compressing', label: 'Compressing' },
  { id: 'writing', label: 'Writing the setup' },
  { id: 'testing', label: 'Checking every byte' },
]

export function Freezing({ status, title, test }: { status: Status; title: string; test: boolean }) {
  const [asking, setAsking] = useState(false)
  const speeds = useRef<number[]>([])
  const last = useRef({ done: 0, at: Date.now(), stage: '' })
  const [speed, setSpeed] = useState(0)
  const [ratios, setRatios] = useState<number[]>([])

  useEffect(() => {
    const now = Date.now()
    const seconds = (now - last.current.at) / 1000
    if (last.current.stage !== status.stage) {
      last.current = { done: status.done, at: now, stage: status.stage }
      speeds.current = []
      setSpeed(0)
      return
    }
    if (seconds < 1) return
    const rate = Math.max(0, status.done - last.current.done) / seconds
    speeds.current = [...speeds.current.slice(-59), rate]
    last.current = { done: status.done, at: now, stage: status.stage }
    const recent = speeds.current.slice(-12)
    setSpeed(recent.reduce((a, b) => a + b, 0) / recent.length)
    if (status.stage === 'compressing' && status.total > 0 && status.done / status.total >= 0.08) {
      setRatios((r) => [...r.slice(-47), ratio(status.packed, status.done)])
    }
  }, [status])

  const compressing = status.stage === 'compressing'
  const share = status.total > 0 ? status.done / status.total : 0
  // What the compressor kept of what it has been fed, and what that makes of the whole game.
  const kept = status.done > 0 ? status.packed / status.done : 0
  const stored = Math.max(0, status.original - status.repeated)
  // The compressors hold data back before they write, so early on this reads low.
  const settled = compressing && share >= 0.08
  const projected = settled ? kept * stored : status.packed
  const overall = status.original > 0 ? projected / status.original : 0
  const left = speed > 0 ? (status.total - status.done) / speed : 0
  const stages = STAGES.filter((s) => test || s.id !== 'testing')
  const at = stages.findIndex((s) => s.id === status.stage)

  return (
    <div className="grid min-h-0 flex-1 grid-cols-[1fr_22rem]">
      <div className="relative min-h-0 min-w-0 border-r border-border">
        <IceGrid done={compressing ? share : status.stage === 'reading' ? 0 : 1} kept={overall || 0.4} className="absolute inset-0 size-full p-6" />
      </div>
      <div className="flex min-h-0 flex-col gap-5 overflow-y-auto p-6">
        <div className="grid gap-1">
          <Caption>{status.paused ? 'Paused' : 'Freezing'}</Caption>
          <h1 className="truncate text-xl font-bold" title={title}>{title}</h1>
        </div>

        <div className="grid gap-1">
          <Caption>{settled ? 'Heading for' : 'Size so far'}</Caption>
          <p className="text-4xl font-bold tabular-nums">{size(projected)}</p>
          <p className="text-xs tabular-nums text-muted-foreground">{settled ? `${(overall * 100).toFixed(1)}% of ${size(status.original)}` : status.original > 0 ? `of ${size(status.original)}` : 'Working it out'}</p>
        </div>

        <AsciiBar fraction={status.total > 0 ? share : null} cells={30} />

        <dl className="grid grid-cols-2 gap-x-4 gap-y-3">
          <Stat label="Ratio so far" value={settled ? `${(kept * 100).toFixed(1)}%` : '...'} />
          <Stat label="Written" value={size(status.packed)} />
          <Stat label="Repeated data" value={status.original > 0 ? size(status.repeated) : '...'} />
          <Stat label="Blocks" value={status.blocksTotal > 0 ? `${status.blocksDone} of ${status.blocksTotal}` : '...'} />
          <Stat label="Speed" value={speed > 0 ? `${size(speed)}/s` : '...'} />
          <Stat label="Time left" value={left > 0 && !status.paused ? duration(left) : '...'} />
          <Stat label="Elapsed" value={duration(status.seconds)} />
        </dl>

        {ratios.length > 1 ? (
          <div className="grid gap-1">
            <Caption>Ratio over time</Caption>
            <AsciiSpark samples={ratios} width={34} />
          </div>
        ) : null}

        <ol className="grid gap-1.5 text-xs">
          {stages.map((stage, i) => (
            <li key={stage.id} className={cn('flex items-center gap-2', i === at ? 'text-foreground' : 'text-muted-foreground')}>
              <span className="grid size-4 place-items-center">
                {i < at ? <CheckIcon className="size-3" aria-hidden="true" /> : <span className={cn('size-1.5', i === at ? 'bg-foreground' : 'bg-border')} />}
              </span>
              {stage.label}
            </li>
          ))}
        </ol>

        <p className="min-h-4 truncate text-[11px] text-muted-foreground" title={status.detail}>{status.detail}</p>

        <div className="mt-auto grid gap-2">
          {asking ? (
            <>
              <Label className="normal-case tracking-normal text-foreground">Stop here?</Label>
              <p className="text-xs text-muted-foreground">Finished blocks are kept. Freeze the same folder again and it picks up from them.</p>
              <div className="grid grid-cols-2 gap-2">
                <Button onClick={() => setAsking(false)}>Keep going</Button>
                <Button variant="danger" onClick={() => void api.stop()}>Stop</Button>
              </div>
            </>
          ) : (
            <div className="grid grid-cols-2 gap-2">
              <Button onClick={() => void api.pause(!status.paused)}>{status.paused ? 'Resume' : 'Pause'}</Button>
              <Button onClick={() => setAsking(true)}>Stop</Button>
            </div>
          )}
        </div>
      </div>
    </div>
  )
}
