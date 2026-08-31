import { readFile } from 'node:fs/promises'
import net from 'node:net'
import process from 'node:process'

const requestedVersion = process.argv[2]
if (!requestedVersion)
  throw new Error('CORE_SELECT_VERSION_REQUIRED: usage pnpm diagnostics:select-core <version>')

const compatibilityRecords = JSON.parse(await readFile(new URL('../src-tauri/resources/core-compatibility.json', import.meta.url), 'utf8'))
const requestedCompatibility = compatibilityRecords.find(record => record.coreVersion === requestedVersion)
if (!requestedCompatibility)
  throw new Error(`CORE_SELECT_UNSUPPORTED_VERSION: ${requestedVersion}`)

const endpointFile = process.env.DSH_DESKTOP_CONTROL_ENDPOINT_FILE
  ?? (process.platform === 'win32' && process.env.APPDATA
    ? `${process.env.APPDATA}\\io.github.hairyf.deepseek-harness-desktop\\control\\endpoint.json`
    : undefined)
if (!endpointFile)
  throw new Error('DSH_DESKTOP_CONTROL_ENDPOINT_FILE is required outside Windows')
const endpoint = JSON.parse(await readFile(endpointFile, 'utf8'))

async function invoke(operation, args = {}) {
  const request = `${JSON.stringify({
    token: endpoint.token,
    operation,
    args,
    traceId: `core-select-${operation}-${Date.now()}`,
  })}\n`
  return await new Promise((resolve, reject) => {
    const socket = net.createConnection({ host: endpoint.host, port: endpoint.port })
    let response = ''
    socket.setEncoding('utf8')
    socket.setTimeout(65_000, () => socket.destroy(new Error(`${operation} timed out`)))
    socket.on('connect', () => socket.end(request))
    socket.on('data', chunk => response += chunk)
    socket.on('end', () => {
      try {
        const parsed = JSON.parse(response)
        if (!parsed.ok)
          reject(new Error(parsed.error ?? `${operation} failed`))
        else
          resolve(parsed.result)
      }
      catch (error) {
        reject(error)
      }
    })
    socket.on('error', reject)
  })
}

const cores = await invoke('core.list')
for (const compatibility of compatibilityRecords) {
  const retained = cores.filter(item => item.source === 'app' && item.version === compatibility.coreVersion)
  if (retained.length !== 1 || !retained[0].present) {
    throw new Error(
      `CORE_SELECT_RETAINED_SET_MISMATCH: version=${compatibility.coreVersion}; rows=${retained.length}; present=${retained[0]?.present === true}`,
    )
  }
}
const core = cores.find(item => item.version === requestedVersion && item.present)
if (!core)
  throw new Error(`CORE_SELECT_NOT_INSTALLED: ${requestedVersion}`)

if (!core.active)
  await invoke('core.activate', { id: core.id })

const deadline = Date.now() + 60_000
let health
let lastError
while (Date.now() < deadline) {
  try {
    health = await invoke('runtime.health')
    break
  }
  catch (error) {
    lastError = error
  }
  await new Promise(resolve => setTimeout(resolve, 500))
}
if (!health)
  throw new Error(`CORE_SELECT_NOT_READY: ${requestedVersion}; ${lastError}`)

let diagnostics
let lastRoute
const routeDeadline = Date.now() + 60_000
while (Date.now() < routeDeadline) {
  diagnostics = await invoke('diagnostics.snapshot')
  lastRoute = diagnostics.webviewRoute
  if (lastRoute?.state === 'ready'
    && lastRoute.coreCompatibility?.coreVersion === requestedVersion
    && diagnostics.surface?.state === 'ready'
    && diagnostics.surface.coreCompatibility?.coreVersion === requestedVersion) {
    break
  }
  await new Promise(resolve => setTimeout(resolve, 250))
}
if (lastRoute?.state !== 'ready'
  || lastRoute.coreCompatibility?.coreVersion !== requestedVersion
  || diagnostics?.surface?.state !== 'ready'
  || diagnostics.surface.coreCompatibility?.coreVersion !== requestedVersion) {
  throw new Error(
    `CORE_SELECT_CLIENT_NOT_READY: requested=${requestedVersion}; route=${lastRoute?.state}; routeCore=${lastRoute?.coreCompatibility?.coreVersion}; surface=${diagnostics?.surface?.state}; surfaceCore=${diagnostics?.surface?.coreCompatibility?.coreVersion}`,
  )
}
if (diagnostics.core.version !== requestedVersion)
  throw new Error(`CORE_SELECT_VERSION_MISMATCH: requested=${requestedVersion}; active=${diagnostics.core.version}`)
const afterCores = await invoke('core.list')
const activeCores = afterCores.filter(item => item.active)
if (activeCores.length !== 1 || activeCores[0].version !== requestedVersion) {
  throw new Error(
    `CORE_SELECT_ACTIVE_LIST_MISMATCH: requested=${requestedVersion}; rows=${activeCores.length}; listed=${activeCores[0]?.version}`,
  )
}
console.log(JSON.stringify({ selected: activeCores[0], health, coreCompatibility: requestedCompatibility }, null, 2))
