import { invoke } from '@tauri-apps/api/core'

export type Level = 'frost' | 'deepFreeze' | 'absoluteZero'
export type Embed = 'auto' | 'always' | 'never'

export type ComponentRule = { id: string; name: string; note: string; selected: boolean; patterns: string[]; size: number }
export type Redist = { name: string; path: string; args: string }
export type Share = { label: string; bytes: number; files: number }
export type LeftOut = { pattern: string; label: string; bytes: number; files: number }
export type ClassStats = { label: string; files: number; original: number; packed: number }

export type Inspection = {
  title: string
  size: number
  files: number
  shares: Share[]
  programs: string[]
  exe: string
  components: ComponentRule[]
  redists: Redist[]
  leftOut: LeftOut[]
}

export type Estimate = { original: number; estimated: number; classes: ClassStats[] }

export type Job = {
  source: string
  outDir: string
  title: string
  version: string
  repacker: string
  website: string
  notes: string
  exe: string
  args: string
  level: Level
  memoryGb: number
  threads: number
  splitMb: number | null
  embed: Embed
  exclude: string[]
  components: ComponentRule[]
  redists: Redist[]
  cover: string | null
  icon: string
  accent: [number, number, number]
  music: string
  test: boolean
}

export type CodecShare = { codec: string; blocks: number; stored: number; packed: number }

export type Report = {
  setup: string | null
  outputs: string[]
  original: number
  packed: number
  totalOutput: number
  seconds: number
  tested: boolean
  duplicateBytes: number
  unwrappedStreams: number
  unwrappedBytes: number
  jpegs: number
  jpegSaved: number
  files: number
  memoryToInstall: number
  codecs: CodecShare[]
  classes: ClassStats[]
  info: string
}

export type Status = {
  running: boolean
  stage: string
  done: number
  total: number
  packed: number
  original: number
  repeated: number
  blocksDone: number
  blocksTotal: number
  detail: string
  paused: boolean
  seconds: number
  cancelled: boolean
  report: Report | null
  error: string | null
}

export type Found = { slug: string; name: string; version: string; cover: string; exe: string; args: string; nsfw: boolean; downloadSize: string }
export type Machine = { memoryGb: number; threads: number; documents: string }

export type LogEntry = {
  title: string
  level: Level
  original: number
  output: number
  seconds: number
  date: string
  folder: string
  setup: string | null
}

export type Saved = {
  repacker?: string
  website?: string
  outDir?: string
  level?: Level
  memoryGb?: number
  log?: LogEntry[]
}

export const api = {
  pickFolder: (title: string) => invoke<string | null>('pick_folder', { title }),
  pickFile: (title: string, kinds: string[]) => invoke<string | null>('pick_file', { title, kinds }),
  inspect: (path: string) => invoke<Inspection>('inspect', { path }),
  estimate: (path: string, exclude: string[], level: Level) => invoke<Estimate>('estimate', { path, exclude, level }),
  start: (job: Job) => invoke<void>('start_freeze', { job }),
  status: () => invoke<Status>('freeze_status'),
  pause: (paused: boolean) => invoke<void>('freeze_pause', { paused }),
  stop: () => invoke<void>('freeze_stop'),
  machine: () => invoke<Machine>('machine'),
  loadState: () => invoke<Saved>('load_state'),
  saveState: (state: Saved) => invoke<void>('save_state', { state }),
  preview: (path: string) => invoke<string>('preview_image', { path }),
  find: (query: string) => invoke<Found[]>('find_on_kryoto', { query }),
  fetchCover: (url: string) => invoke<string>('fetch_cover', { url }),
  openFolder: (path: string) => invoke<void>('open_folder', { path }),
  runSetup: (path: string) => invoke<void>('run_setup', { path }),
  readText: (path: string) => invoke<string>('read_text', { path }),
}

export function size(bytes: number): string {
  if (bytes >= 1e12) return `${(bytes / 1e12).toFixed(2)} TB`
  if (bytes >= 1e9) return `${(bytes / 1e9).toFixed(2)} GB`
  if (bytes >= 1e6) return `${(bytes / 1e6).toFixed(1)} MB`
  return `${Math.round(bytes / 1e3)} KB`
}

export function duration(seconds: number): string {
  const s = Math.max(0, Math.round(seconds))
  if (s >= 3600) return `${Math.floor(s / 3600)}h ${String(Math.floor((s % 3600) / 60)).padStart(2, '0')}m`
  if (s >= 60) return `${Math.floor(s / 60)}m ${String(s % 60).padStart(2, '0')}s`
  return `${s}s`
}

/** Packed size as a share of the original, the way repackers quote it. */
export const ratio = (packed: number, original: number) => (original > 0 ? (packed / original) * 100 : 0)

/** `428 MB` from a kryo.to game card, in bytes. */
export function parseSize(text: string): number | null {
  const match = /([\d.]+)\s*(KB|MB|GB|TB)/i.exec(text)
  if (!match) return null
  const unit = { KB: 1e3, MB: 1e6, GB: 1e9, TB: 1e12 }[match[2]!.toUpperCase() as 'KB' | 'MB' | 'GB' | 'TB']
  return Number(match[1]) * unit
}
