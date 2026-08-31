import { readdirSync, readFileSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'

const dest = process.argv[2]
if (!dest)
  throw new Error('ALPHA1_ARTIFACT_DEST_REQUIRED')

const manifestPath = join(dest, 'package.json')
const manifest = JSON.parse(readFileSync(manifestPath, 'utf8'))

function writeManifest() {
  writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`)
}

function adaptSplitClient() {
  const inject = manifest.dsh?.client?.inject
  if (inject !== undefined) {
    manifest.dsh.client.inject = [...new Set(inject.map(name =>
      name === '@deepseek-ai/dsh-client-runtime' ? '@deepseek-ai/dsh-client-store' : name,
    ))]
  }
  for (const [name, range] of Object.entries(manifest.dependencies ?? {})) {
    if (!name.startsWith('@deepseek-ai/dsh-'))
      continue
    delete manifest.dependencies[name]
    manifest.peerDependencies ??= {}
    manifest.peerDependencies[name] = range
  }

  function adaptTree(dir) {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const path = join(dir, entry.name)
      if (entry.isDirectory()) {
        if (entry.name !== 'node_modules') adaptTree(path)
        continue
      }
      if (!entry.isFile() || !entry.name.endsWith('.js')) continue
      const source = readFileSync(path, 'utf8')
      const adapted = source.replaceAll('@deepseek-ai/dsh-client-runtime/client', '@deepseek-ai/dsh-client-store')
      if (adapted !== source) writeFileSync(path, `${adapted.trimEnd()}\n`)
      if (adapted.includes('@deepseek-ai/dsh-client-runtime/client'))
        throw new Error(`${path}: alpha split-client adaptation is incomplete`)
    }
  }
  adaptTree(dest)
}

function adaptSession() {
  if (manifest.name !== 'dsh-tauri-session') return
  const clientPath = join(dest, 'dist', 'client.js')
  const source = readFileSync(clientPath, 'utf8')
  const directRegistration = /([A-Za-z_$][\w$]*)\.effect\(\(\)=>\1\.slots\.register\((\{name:`settings\.section`,[\s\S]*?\},[A-Za-z_$][\w$]*)\),[A-Za-z_$][\w$]*\)/
  let adapted = source.replace(directRegistration, '$1.slots.inject("settings.section",()=>$1.slots.register($2))')
  if (adapted === source)
    throw new Error(`${dest}: alpha session slot adaptation did not match`)
  const legacyEffect = /,([A-Za-z_$][\w$]*)\.effect\(\(\)=>[A-Za-z_$][\w$]*\(\1\.workspaces,\1\.sessions\),[A-Za-z_$][\w$]*\)(?=\})/
  const withoutLegacyArchive = adapted.replace(legacyEffect, '')
  if (withoutLegacyArchive === adapted)
    throw new Error(`${dest}: alpha session archive adaptation did not match`)
  writeFileSync(clientPath, `${withoutLegacyArchive.trimEnd()}\n`)
}

function adaptPanel() {
  if (manifest.name !== 'dsh-tauri-panel') return
  const clientPath = join(dest, 'dist', 'client.js')
  const source = readFileSync(clientPath, 'utf8')
  const legacyCall = /([A-Za-z_$][\w$]*)\.workspaces\.startSession\(/g
  const legacyCalls = [...source.matchAll(legacyCall)]
  if (legacyCalls.length !== 1)
    throw new Error(`${clientPath}: expected one RC workspace navigation call, found ${legacyCalls.length}`)
  let adapted = source.replace(legacyCall, '$1.get(`uiWorkspace`).startSession(')
  const legacyInject = /([A-Za-z_$][\w$]*=\[`slots`,`layout`,`workspaces`),`locale`\]/g
  const injectLists = [...adapted.matchAll(legacyInject)]
  if (injectLists.length !== 1)
    throw new Error(`${clientPath}: expected one RC Panel injection list, found ${injectLists.length}`)
  adapted = adapted.replace(legacyInject, '$1,`uiWorkspace`,`locale`]')
  if (adapted.includes('.workspaces.startSession(') || !adapted.includes('.get(`uiWorkspace`).startSession('))
    throw new Error(`${clientPath}: alpha workspace navigation adaptation is incomplete`)
  manifest.dsh ??= {}
  manifest.dsh.client ??= {}
  manifest.dsh.client.inject = [...new Set([
    ...(manifest.dsh.client.inject ?? []),
    '@deepseek-ai/dsh-client-ui-workspace',
  ])]
  writeFileSync(clientPath, `${adapted.trimEnd()}\n`)
}

adaptSplitClient()
adaptSession()
adaptPanel()
writeManifest()
