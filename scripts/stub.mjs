// Builds the setup program and puts it where the app embeds it from.
import { spawnSync } from 'node:child_process'
import { copyFileSync } from 'node:fs'
import path from 'node:path'
import { root, tauriEnv } from './tauri-env.mjs'

const env = tauriEnv()
const run = spawnSync('cargo', ['build', '--release', '-p', 'kryoto-setup'], { cwd: root, env, stdio: 'inherit', shell: true })
if (run.status !== 0) process.exit(run.status ?? 1)
const target = env.CARGO_TARGET_DIR || path.join(root, 'target')
copyFileSync(path.join(target, 'release', 'kryoto-setup.exe'), path.join(root, 'src-tauri', 'stub', 'kryoto-setup.exe'))
