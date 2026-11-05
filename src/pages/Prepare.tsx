import { useEffect, useMemo, useState, type ReactNode } from 'react'
import { Busy, Button, Caption, Check, Dropdown, Label, Segmented, inputCls } from '@/ui'
import { ArrowLeft, ImagePlus, Search, X } from '@/ui/pixel-icons'
import { api, ratio, size, type Estimate, type Found, type Inspection, type Job, type Machine, type Saved } from '@/lib/api'
import { ACCENTS, LEVELS, SPLITS } from '@/lib/levels'
import { cn } from '@/lib/utils'

function Field({ label, children, hint }: { label: string; children: ReactNode; hint?: string }) {
  return (
    <label className="grid gap-1.5">
      <Caption>{label}</Caption>
      {children}
      {hint ? <span className="text-[11px] text-muted-foreground">{hint}</span> : null}
    </label>
  )
}

function Group({ title, children, aside }: { title: string; children: ReactNode; aside?: ReactNode }) {
  return (
    <section className="kryo-radius grid gap-3 border border-border p-4">
      <div className="flex items-center justify-between gap-3">
        <Label>{title}</Label>
        {aside}
      </div>
      {children}
    </section>
  )
}

/** The folder by kind of file, as one stacked strip. */
function ShareBar({ inspection }: { inspection: Inspection }) {
  const shades = [1, 0.72, 0.5, 0.34, 0.22, 0.14]
  return (
    <div className="grid gap-2">
      <div className="flex h-3 w-full overflow-hidden" role="img" aria-label="Size of the folder by kind of file">
        {inspection.shares.map((share, i) => (
          <div key={share.label} className="bg-foreground" style={{ width: `${(share.bytes / Math.max(1, inspection.size)) * 100}%`, opacity: shades[i % shades.length] }} />
        ))}
      </div>
      <dl className="grid grid-cols-2 gap-x-6 gap-y-1 text-xs">
        {inspection.shares.map((share, i) => (
          <div key={share.label} className="flex items-center gap-2">
            <span className="size-2 shrink-0 bg-foreground" style={{ opacity: shades[i % shades.length] }} aria-hidden="true" />
            <dt className="min-w-0 flex-1 truncate text-muted-foreground">{share.label}</dt>
            <dd className="m-0 tabular-nums">{size(share.bytes)}</dd>
          </div>
        ))}
      </dl>
    </div>
  )
}

export function Prepare({ folder, inspection, machine, saved, onBack, onFreeze }: {
  folder: string
  inspection: Inspection
  machine: Machine
  saved: Saved
  onBack: () => void
  onFreeze: (job: Job, kryotoSize: string | null) => void
}) {
  const [job, setJob] = useState<Job>(() => ({
    source: folder,
    outDir: saved.outDir || machine.documents,
    title: inspection.title,
    version: '',
    repacker: saved.repacker ?? '',
    website: saved.website ?? '',
    notes: '',
    exe: inspection.exe,
    args: '',
    level: saved.level ?? 'deepFreeze',
    memoryGb: saved.memoryGb ?? Math.max(2, Math.floor(machine.memoryGb / 2)),
    threads: machine.threads,
    splitMb: null,
    embed: 'auto',
    exclude: [],
    components: inspection.components,
    redists: inspection.redists,
    cover: null,
    icon: 'game',
    accent: ACCENTS[0]!.rgb,
    music: 'tune',
    test: true,
  }))
  const set = <K extends keyof Job>(key: K, value: Job[K]) => setJob((j) => ({ ...j, [key]: value }))

  const [preview, setPreview] = useState<string | null>(null)
  const [query, setQuery] = useState(inspection.title)
  const [found, setFound] = useState<Found[] | null>(null)
  const [searching, setSearching] = useState(false)
  const [kryotoSize, setKryotoSize] = useState<string | null>(null)
  const [problem, setProblem] = useState<string | null>(null)
  const [estimate, setEstimate] = useState<Estimate | null>(null)
  const [estimating, setEstimating] = useState(false)

  const leaving = useMemo(() => inspection.leftOut.filter((row) => job.exclude.includes(row.pattern)).reduce((n, row) => n + row.bytes, 0), [inspection, job.exclude])

  // A new mode or a changed list makes the old guess wrong.
  useEffect(() => setEstimate(null), [job.level, job.exclude])

  const search = async () => {
    setSearching(true)
    setProblem(null)
    try {
      setFound(await api.find(query))
    } catch (e) {
      setProblem(String(e))
    }
    setSearching(false)
  }

  const useFound = async (game: Found) => {
    setFound(null)
    setKryotoSize(game.downloadSize || null)
    setJob((j) => ({
      ...j,
      title: game.name,
      version: game.version || j.version,
      exe: game.exe && inspection.programs.includes(game.exe) ? game.exe : j.exe,
      args: game.args || j.args,
    }))
    if (game.cover) {
      try {
        const data = await api.fetchCover(game.cover)
        set('cover', data)
        setPreview(await api.preview(data))
      } catch (e) {
        setProblem(String(e))
      }
    }
  }

  const pickCover = async () => {
    const path = await api.pickFile('Pick the cover art', ['png', 'jpg', 'jpeg', 'webp', 'bmp'])
    if (!path) return
    try {
      setPreview(await api.preview(path))
      set('cover', path)
    } catch (e) {
      setProblem(String(e))
    }
  }

  const pickMusic = async () => {
    const path = await api.pickFile('Pick the setup music', ['wav'])
    if (path) set('music', path)
  }

  const pickIcon = async () => {
    const path = await api.pickFile('Pick the icon', ['ico'])
    if (path) set('icon', path)
  }

  const pickOut = async () => {
    const path = await api.pickFolder('Where should the repack go?')
    if (path) set('outDir', path)
  }

  const runEstimate = async () => {
    setEstimating(true)
    setProblem(null)
    try {
      setEstimate(await api.estimate(folder, job.exclude, job.level))
    } catch (e) {
      setProblem(String(e))
    }
    setEstimating(false)
  }

  const memories = [2, 4, 6, 8, 12, 16, 24, 32, 48, 64].filter((g) => g < machine.memoryGb)
  const threads = [1, 2, 4, 6, 8, 12, 16, 24, 32].filter((t) => t < machine.threads).concat(machine.threads)
  const ready = job.title.trim() !== '' && job.outDir.trim() !== ''

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="min-h-0 flex-1 overflow-y-auto px-6 py-5">
        <div className="mx-auto grid w-full max-w-5xl gap-4 lg:grid-cols-2">
          <div className="grid content-start gap-4">
            <Group title="The game" aside={<Caption>{size(inspection.size)} · {inspection.files} files</Caption>}>
              <div className="flex gap-2">
                <input aria-label="Search Kryoto" className={inputCls} value={query} onChange={(e) => setQuery(e.target.value)} onKeyDown={(e) => e.key === 'Enter' && void search()} placeholder="Find this game on Kryoto" />
                <Button disabled={searching || !query.trim()} onClick={() => void search()}>
                  <Search className="size-3.5" aria-hidden="true" />
                  Find
                </Button>
              </div>
              {found ? (
                <ul className="grid gap-1">
                  {found.length === 0 ? <li className="text-xs text-muted-foreground">Not on Kryoto.</li> : null}
                  {found.map((game) => (
                    <li key={game.slug}>
                      <button type="button" className="kryo-radius flex w-full items-center gap-3 px-2 py-1.5 text-left text-xs hover:bg-secondary" onClick={() => void useFound(game)}>
                        <span className="min-w-0 flex-1 truncate">{game.name}</span>
                        <span className="shrink-0 text-muted-foreground">{game.downloadSize}</span>
                      </button>
                    </li>
                  ))}
                </ul>
              ) : null}
              <div className="grid grid-cols-[1fr_8rem] gap-3">
                <Field label="Title"><input className={inputCls} value={job.title} onChange={(e) => set('title', e.target.value)} /></Field>
                <Field label="Version"><input className={inputCls} value={job.version} onChange={(e) => set('version', e.target.value)} /></Field>
              </div>
              <Field label="Starts with">
                {inspection.programs.length > 0 ? (
                  <Dropdown value={job.exe} onChange={(v) => set('exe', v)} label="Program to start" options={inspection.programs.map((p) => ({ value: p, label: p }))} />
                ) : (
                  <input className={inputCls} value={job.exe} onChange={(e) => set('exe', e.target.value)} />
                )}
              </Field>
              <Field label="Launch options"><input className={inputCls} value={job.args} onChange={(e) => set('args', e.target.value)} placeholder="None" /></Field>
              <div className="grid grid-cols-2 gap-3">
                <Field label="Repacked by"><input className={inputCls} value={job.repacker} onChange={(e) => set('repacker', e.target.value)} /></Field>
                <Field label="Website"><input className={inputCls} value={job.website} onChange={(e) => set('website', e.target.value)} placeholder="https://kryo.to" /></Field>
              </div>
              <Field label="Note after install" hint="Shown on the last page of the setup.">
                <textarea className={cn(inputCls, 'kryo-radius h-16 resize-none py-2')} value={job.notes} onChange={(e) => set('notes', e.target.value)} />
              </Field>
            </Group>

            <Group title="What's in it">
              <ShareBar inspection={inspection} />
            </Group>

            {inspection.leftOut.length > 0 ? (
              <Group title="Leave out" aside={leaving > 0 ? <Caption>{size(leaving)} less</Caption> : undefined}>
                {inspection.leftOut.map((row) => (
                  <Check
                    key={row.pattern}
                    checked={job.exclude.includes(row.pattern)}
                    onChange={(on) => set('exclude', on ? [...job.exclude, row.pattern] : job.exclude.filter((p) => p !== row.pattern))}
                    label={`${row.label}  ·  ${row.files} files, ${size(row.bytes)}`}
                  />
                ))}
              </Group>
            ) : null}

            {job.components.length > 0 ? (
              <Group title="Optional parts">
                <p className="text-[11px] text-muted-foreground">Each goes in its own file, so players can skip the download.</p>
                {job.components.map((part, i) => (
                  <Check
                    key={part.id}
                    checked={part.selected}
                    onChange={(on) => set('components', job.components.map((c, n) => (n === i ? { ...c, selected: on } : c)))}
                    label={`${part.name}  ·  ${size(part.size)}, ${part.selected ? 'ticked' : 'not ticked'} by default`}
                  />
                ))}
              </Group>
            ) : null}
          </div>

          <div className="grid content-start gap-4">
            <Group title="How hard">
              <Segmented value={job.level} onChange={(v) => set('level', v)} label="Compression" options={LEVELS.map(({ value, label }) => ({ value, label }))} />
              <p className="text-xs text-muted-foreground">{LEVELS.find((l) => l.value === job.level)?.note}</p>
              <div className="grid grid-cols-2 gap-3">
                <Field label="Memory">
                  <Dropdown value={String(job.memoryGb)} onChange={(v) => set('memoryGb', Number(v))} label="Memory" options={memories.map((g) => ({ value: String(g), label: `${g} GB of ${machine.memoryGb}` }))} />
                </Field>
                <Field label="Threads">
                  <Dropdown value={String(job.threads)} onChange={(v) => set('threads', Number(v))} label="Threads" options={threads.map((t) => ({ value: String(t), label: String(t) }))} />
                </Field>
              </div>
              <div className="flex items-center justify-between gap-3 border-t border-border pt-3">
                {estimate ? (
                  <dl className="flex gap-6 text-xs">
                    <div><dt className="text-[10px] uppercase tracking-wider text-muted-foreground">About</dt><dd className="m-0 text-sm font-bold tabular-nums">{size(estimate.estimated)}</dd></div>
                    <div><dt className="text-[10px] uppercase tracking-wider text-muted-foreground">Ratio</dt><dd className="m-0 text-sm font-bold tabular-nums">{ratio(estimate.estimated, estimate.original).toFixed(1)}%</dd></div>
                  </dl>
                ) : (
                  <p className="text-[11px] text-muted-foreground">Tries a sample of each kind of file. Repeated data usually beats it.</p>
                )}
                {estimating ? <Busy label="Estimating" /> : <Button size="sm" onClick={() => void runEstimate()}>Estimate size</Button>}
              </div>
              {estimate ? (
                <table className="w-full text-xs">
                  <tbody>
                    {estimate.classes.map((row) => (
                      <tr key={row.label}>
                        <td className="py-0.5 text-muted-foreground">{row.label}</td>
                        <td className="py-0.5 text-right tabular-nums">{size(row.packed)}</td>
                        <td className="w-14 py-0.5 text-right tabular-nums text-muted-foreground">{ratio(row.packed, row.original).toFixed(0)}%</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              ) : null}
            </Group>

            <Group title="The setup">
              <div className="flex gap-4">
                <button type="button" onClick={() => void pickCover()} className="kryo-radius grid h-36 w-24 shrink-0 place-items-center overflow-hidden border border-border text-muted-foreground hover:border-foreground" aria-label="Pick the cover art">
                  {preview ? <img src={preview} alt="" className="size-full object-cover" /> : <ImagePlus className="size-5" aria-hidden="true" />}
                </button>
                <div className="grid min-w-0 flex-1 content-start gap-3">
                  <Field label="Accent">
                    <div className="flex flex-wrap gap-1.5">
                      {ACCENTS.map((accent) => (
                        <button
                          key={accent.name}
                          type="button"
                          title={accent.name}
                          aria-label={accent.name}
                          aria-pressed={accent.rgb.join() === job.accent.join()}
                          onClick={() => set('accent', accent.rgb)}
                          className={cn('size-6 border-2', accent.rgb.join() === job.accent.join() ? 'border-foreground' : 'border-transparent')}
                          style={{ background: `rgb(${accent.rgb.join(' ')})` }}
                        />
                      ))}
                    </div>
                  </Field>
                  <div className="grid gap-1.5">
                    <Caption>Music</Caption>
                    <div className="flex flex-wrap items-center gap-2">
                      <Segmented value={job.music === 'tune' || job.music === 'none' ? job.music : 'own'} onChange={(v) => (v === 'own' ? void pickMusic() : set('music', v))} label="Setup music" options={[{ value: 'tune', label: 'Kryoto tune' }, { value: 'none', label: 'Off' }, { value: 'own', label: 'My .wav' }]} />
                    </div>
                  </div>
                  <div className="grid gap-1.5">
                    <Caption>Icon</Caption>
                    <Segmented value={job.icon === 'game' || job.icon === 'kryoto' ? job.icon : 'own'} onChange={(v) => (v === 'own' ? void pickIcon() : set('icon', v))} label="Setup icon" options={[{ value: 'game', label: "The game's" }, { value: 'kryoto', label: 'K//' }, { value: 'own', label: 'My .ico' }]} />
                  </div>
                </div>
              </div>
              {job.cover ? (
                <button type="button" className="flex w-fit items-center gap-1 text-[11px] text-muted-foreground hover:text-foreground" onClick={() => { set('cover', null); setPreview(null) }}>
                  <X className="size-3" aria-hidden="true" />
                  Use the K// instead
                </button>
              ) : <p className="text-[11px] text-muted-foreground">No cover? The setup freezes the K// instead.</p>}
            </Group>

            <Group title="Output">
              <div className="flex gap-2">
                <input aria-label="Output folder" className={inputCls} value={job.outDir} onChange={(e) => set('outDir', e.target.value)} />
                <Button onClick={() => void pickOut()}>Browse</Button>
              </div>
              <div className="grid grid-cols-2 gap-3">
                <Field label="Data">
                  <Dropdown value={job.embed} onChange={(v) => set('embed', v)} label="Where the data goes" options={[{ value: 'auto', label: 'Inside the setup if it fits' }, { value: 'never', label: 'Beside it, as .kryo' }]} />
                </Field>
                <Field label="Split">
                  <Dropdown value={String(job.splitMb ?? 0)} onChange={(v) => { const mb = Number(v); set('splitMb', mb || null) }} label="Part size" options={SPLITS} />
                </Field>
              </div>
              <Check checked={job.test} onChange={(on) => set('test', on)} label="Check every byte against the original afterwards" />
            </Group>
          </div>
        </div>
      </div>

      <footer className="flex shrink-0 items-center gap-3 border-t border-border px-6 py-3">
        <Button variant="ghost" onClick={onBack}>
          <ArrowLeft className="size-3.5" aria-hidden="true" />
          Back
        </Button>
        {problem ? <p role="alert" className="min-w-0 flex-1 truncate text-xs text-destructive" title={problem}>{problem}</p> : <span className="flex-1" />}
        <Button variant="primary" size="lg" disabled={!ready} onClick={() => onFreeze(job, kryotoSize)}>Freeze</Button>
      </footer>
    </div>
  )
}
