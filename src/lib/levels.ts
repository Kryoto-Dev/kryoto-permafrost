import type { Level } from './api'

export const LEVELS: { value: Level; label: string; note: string }[] = [
  { value: 'frost', label: 'Frost', note: 'Quick. Good for checking a repack before the long run.' },
  { value: 'deepFreeze', label: 'Deep Freeze', note: 'A strong result in a sensible time.' },
  { value: 'absoluteZero', label: 'Absolute Zero', note: 'The smallest it can go. Slow, and takes all the memory you give it.' },
]

/** The site's palettes, as the setup's accent. Ice is Permafrost's own. */
export const ACCENTS: { name: string; rgb: [number, number, number] }[] = [
  { name: 'Ice', rgb: [159, 211, 238] },
  { name: 'Mono', rgb: [237, 237, 237] },
  { name: 'Amber', rgb: [255, 196, 90] },
  { name: 'Emerald', rgb: [110, 231, 170] },
  { name: 'Nord', rgb: [136, 192, 208] },
  { name: 'Sepia', rgb: [214, 186, 140] },
  { name: 'Blossom', rgb: [244, 168, 200] },
]

/** `.kryo` part sizes, for hosts with an upload limit. */
export const SPLITS: { value: string; label: string }[] = [
  { value: '0', label: 'One file' },
  { value: '1024', label: '1 GB parts' },
  { value: '2048', label: '2 GB parts' },
  { value: '4096', label: '4 GB parts' },
  { value: '8192', label: '8 GB parts' },
  { value: '16384', label: '16 GB parts' },
]
