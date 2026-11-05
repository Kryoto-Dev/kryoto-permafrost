// `pnpm app:dev` and `pnpm app:build`: the setup program first, then the app around it.
import { spawnSync } from 'node:child_process'
import { existsSync } from 'node:fs'
import path from 'node:path'
import { root, tauriEnv } from './tauri-env.mjs'

const mode = process.argv[2]
const stub = path.join(root, 'src-tauri', 'stub', 'kryoto-setup.exe')
if (mode === 'build' || !existsSync(stub)) {
  const built = spawnSync('node', ['scripts/stub.mjs'], { cwd: root, stdio: 'inherit' })
  if (built.status !== 0) process.exit(built.status ?? 1)
}
const args = mode === 'build' ? ['tauri', 'build', '--no-bundle'] : ['tauri', 'dev']
const run = spawnSync('pnpm', args, { cwd: root, env: tauriEnv(), stdio: 'inherit', shell: true })
process.exit(run.status ?? 1)
