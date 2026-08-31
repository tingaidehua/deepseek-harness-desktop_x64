import { readFileSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'

const dest = process.argv[2]
if (!dest)
  throw new Error('RC2_ARTIFACT_DEST_REQUIRED')

const manifestPath = join(dest, 'package.json')
const manifest = JSON.parse(readFileSync(manifestPath, 'utf8'))
if (manifest.name === 'dsh-win-terminal-inspector') {
  manifest.dsh = { bundle: { patch: './cordis.patch.yml' } }
  writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`)
  writeFileSync(join(dest, 'cordis.patch.yml'), '- insert:\n    - id: win-terminal-inspector\n      name: dsh-win-terminal-inspector\n')
}
