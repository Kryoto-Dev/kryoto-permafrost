import { useCallback, useEffect, useRef, useState } from 'react'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { getCurrentWebview } from '@tauri-apps/api/webview'
import { Maximize2, Minus, X } from '@/ui/pixel-icons'
import { KryoMark } from '@/ui/ascii/KryoMark'
import { api, type Inspection, type Job, type LogEntry, type Machine, type Report, type Saved, type Status } from '@/lib/api'
import { Start } from '@/pages/Start'
import { Prepare } from '@/pages/Prepare'
import { Freezing } from '@/pages/Freezing'
import { Result } from '@/pages/Result'

type View =
  | { page: 'start' }
  | { page: 'prepare'; folder: string; inspection: Inspection }
  | { page: 'freezing'; job: Job; kryotoSize: string | null }
  | { page: 'result'; report: Report; title: string; kryotoSize: string | null }

const IDLE: Status = {
  running: true, stage: 'scanning', done: 0, total: 0, packed: 0, original: 0, repeated: 0,
  blocksDone: 0, blocksTotal: 0, detail: '', paused: false, seconds: 0, cancelled: false, report: null, error: null,
}

export function App() {
  const [view, setView] = useState<View>({ page: 'start' })
  const [machine, setMachine] = useState<Machine | null>(null)
  const [saved, setSaved] = useState<Saved>({})
  const [status, setStatus] = useState<Status>(IDLE)
  const [hovering, setHovering] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const page = useRef(view.page)
  page.current = view.page

  useEffect(() => {
    void api.machine().then(setMachine)
    void api.loadState().then(setSaved)
  }, [])

  const save = useCallback((next: Saved) => {
    setSaved(next)
    void api.saveState(next)
  }, [])

  const open = useCallback(async (folder: string) => {
    setBusy(true)
    setError(null)
    try {
      const inspection = await api.inspect(folder)
      setView({ page: 'prepare', folder, inspection })
    } catch (e) {
      setError(String(e))
    }
    setBusy(false)
  }, [])

  useEffect(() => {
    const stop = getCurrentWebview().onDragDropEvent((event) => {
      if (page.current !== 'start') return
      if (event.payload.type === 'enter' || event.payload.type === 'over') setHovering(true)
      else setHovering(false)
      if (event.payload.type === 'drop' && event.payload.paths[0]) void open(event.payload.paths[0])
    })
    return () => void stop.then((f) => f())
  }, [open])

  // While freezing, ask the native side how it is going.
  const freezing = view.page === 'freezing' ? view : null
  useEffect(() => {
    if (!freezing) return
    let alive = true
    const tick = async () => {
      const now = await api.status()
      if (!alive) return
      setStatus(now)
      if (now.report) {
        const entry: LogEntry = {
          title: freezing.job.title,
          level: freezing.job.level,
          original: now.report.original,
          output: now.report.totalOutput,
          seconds: now.report.seconds,
          date: new Date().toISOString(),
          folder: freezing.job.outDir,
          setup: now.report.setup,
        }
        save({
          ...saved,
          repacker: freezing.job.repacker,
          website: freezing.job.website,
          outDir: freezing.job.outDir,
          level: freezing.job.level,
          memoryGb: freezing.job.memoryGb,
          log: [entry, ...(saved.log ?? [])].slice(0, 100),
        })
        setView({ page: 'result', report: now.report, title: freezing.job.title, kryotoSize: freezing.kryotoSize })
      } else if (now.error) {
        setError(now.error)
        setView({ page: 'start' })
      } else if (now.cancelled) {
        setError('Stopped. Freeze the same folder again to pick up from the finished blocks.')
        setView({ page: 'start' })
      }
    }
    const timer = window.setInterval(() => void tick(), 400)
    return () => {
      alive = false
      window.clearInterval(timer)
    }
  }, [freezing, save, saved])

  const startFreeze = async (job: Job, kryotoSize: string | null) => {
    try {
      setStatus(IDLE)
      await api.start(job)
      setView({ page: 'freezing', job, kryotoSize })
    } catch (e) {
      setError(String(e))
      setView({ page: 'start' })
    }
  }

  const win = getCurrentWindow()
  const icon = 'grid size-7 place-items-center text-muted-foreground transition-colors hover:text-foreground'

  return (
    <main className="flex h-full flex-col bg-background text-foreground">
      <header className="drag flex h-11 shrink-0 items-center gap-3 border-b border-border px-4" onDoubleClick={() => void win.toggleMaximize()}>
        <KryoMark className="h-4" />
        <span className="text-[10px] uppercase tracking-[0.25em] text-muted-foreground">Permafrost</span>
        <span className="flex-1" />
        <div className="no-drag flex items-center gap-1">
          <button type="button" aria-label="Minimise" className={icon} onClick={() => void win.minimize()}><Minus className="size-4" aria-hidden="true" /></button>
          <button type="button" aria-label="Maximise" className={icon} onClick={() => void win.toggleMaximize()}><Maximize2 className="size-3.5" aria-hidden="true" /></button>
          <button type="button" aria-label="Close" className={icon} onClick={() => void win.close()}><X className="size-4" aria-hidden="true" /></button>
        </div>
      </header>

      {view.page === 'start' ? (
        <div className="min-h-0 flex-1 overflow-y-auto">
          <Start log={saved.log ?? []} hovering={hovering} busy={busy} error={error} onPick={(folder) => void open(folder)} />
        </div>
      ) : null}
      {view.page === 'prepare' && machine ? (
        <Prepare folder={view.folder} inspection={view.inspection} machine={machine} saved={saved} onBack={() => setView({ page: 'start' })} onFreeze={(job, kryotoSize) => void startFreeze(job, kryotoSize)} />
      ) : null}
      {view.page === 'freezing' ? <Freezing status={status} title={view.job.title} test={view.job.test} /> : null}
      {view.page === 'result' ? <Result report={view.report} title={view.title} kryotoSize={view.kryotoSize} onAgain={() => setView({ page: 'start' })} /> : null}
    </main>
  )
}
