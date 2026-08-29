import { readFile } from 'node:fs/promises'

const requestedVersion = process.argv[2]
if (!requestedVersion)
  throw new Error('CORE_SELECT_VERSION_REQUIRED: usage pnpm diagnostics:select-core <version>')

const cdpPort = process.env.DSH_DESKTOP_CDP_PORT || '9337'
const compatibilityRecords = JSON.parse(await readFile(new URL('../src-tauri/resources/core-compatibility.json', import.meta.url), 'utf8'))
const requestedCompatibility = compatibilityRecords.find(record => record.coreVersion === requestedVersion)
if (!requestedCompatibility)
  throw new Error(`CORE_SELECT_UNSUPPORTED_VERSION: ${requestedVersion}`)
const targets = await (await fetch(`http://127.0.0.1:${cdpPort}/json/list`)).json()
const target = targets.find(item => item.type === 'page')
if (!target)
  throw new Error('CORE_SELECT_TARGET_MISSING: start Desktop with WebView2 remote debugging enabled')

const socket = new WebSocket(target.webSocketDebuggerUrl)
const pending = new Map()
const contexts = []
let sequence = 0
socket.addEventListener('message', (event) => {
  const message = JSON.parse(event.data)
  if (message.method === 'Runtime.executionContextCreated') contexts.push(message.params.context)
  if (!message.id || !pending.has(message.id)) return
  const operation = pending.get(message.id)
  pending.delete(message.id)
  if (message.error) operation.reject(new Error(JSON.stringify(message.error)))
  else operation.resolve(message.result)
})
await new Promise((resolve, reject) => {
  socket.addEventListener('open', resolve, { once: true })
  socket.addEventListener('error', reject, { once: true })
})

function call(method, params = {}) {
  const id = ++sequence
  socket.send(JSON.stringify({ id, method, params }))
  return new Promise((resolve, reject) => pending.set(id, { resolve, reject }))
}

await call('Runtime.enable')
await new Promise(resolve => setTimeout(resolve, 250))
const appContext = contexts.find(item =>
  item.origin === 'http://tauri.localhost' || item.origin.startsWith('tauri://'))
if (!appContext)
  throw new Error(`CORE_SELECT_APP_CONTEXT_MISSING: ${JSON.stringify(contexts)}`)

async function evaluate(expression, allowFailure = false) {
  const answer = await call('Runtime.evaluate', {
    contextId: appContext.id,
    expression,
    awaitPromise: true,
    returnByValue: true,
  })
  if (answer.exceptionDetails) {
    if (allowFailure) return undefined
    throw new Error(`CORE_SELECT_EVALUATION: ${answer.exceptionDetails.exception?.description || answer.exceptionDetails.text}`)
  }
  return answer.result.value
}

const cores = await evaluate('globalThis.__TAURI_INTERNALS__.invoke("get_cores")')
const core = cores.find(item => item.version === requestedVersion && item.present)
if (!core)
  throw new Error(`CORE_SELECT_NOT_INSTALLED: ${requestedVersion}`)

await evaluate(`globalThis.__TAURI_INTERNALS__.invoke("set_active_core", { id: ${JSON.stringify(core.id)} })`)
await evaluate('globalThis.__TAURI_INTERNALS__.invoke("restart_harness")')

const deadline = Date.now() + 60_000
let health
while (Date.now() < deadline) {
  health = await evaluate('globalThis.__TAURI_INTERNALS__.invoke("proxy_health_check")', true)
  if (health !== undefined) break
  await new Promise(resolve => setTimeout(resolve, 500))
}
if (health === undefined)
  throw new Error(`CORE_SELECT_NOT_READY: ${requestedVersion}`)

const diagnostics = await evaluate('globalThis.__TAURI_INTERNALS__.invoke("get_diagnostics_snapshot")')
if (diagnostics.core.version !== requestedVersion)
  throw new Error(`CORE_SELECT_VERSION_MISMATCH: requested=${requestedVersion}; active=${diagnostics.core.version}`)
console.log(JSON.stringify({ selected: core, health, coreCompatibility: requestedCompatibility }, null, 2))
await evaluate('location.reload(); true', true)
await new Promise(resolve => setTimeout(resolve, 1_500))
socket.close()
