import { mkdir } from 'node:fs/promises'
import { resolve } from 'node:path'
import process from 'node:process'

const cdpPort = process.env.DSH_DESKTOP_CDP_PORT || '9337'
const targets = await (await fetch(`http://127.0.0.1:${cdpPort}/json/list`)).json()
const target = targets.find(item => item.type === 'page')
if (!target)
  throw new Error('SURFACE_FIXTURE_TARGET_MISSING: start Desktop with WebView2 remote debugging enabled')

const socket = new WebSocket(target.webSocketDebuggerUrl)
const pending = new Map()
const contexts = []
let sequence = 0
socket.addEventListener('message', (event) => {
  const message = JSON.parse(event.data)
  if (message.method === 'Runtime.executionContextCreated')
    contexts.push(message.params.context)
  if (!message.id || !pending.has(message.id))
    return
  const operation = pending.get(message.id)
  pending.delete(message.id)
  if (message.error)
    operation.reject(new Error(JSON.stringify(message.error)))
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
await new Promise(resolve => setTimeout(resolve, 150))
const appContext = contexts.find(item =>
  item.origin === 'http://tauri.localhost'
  || item.origin === 'http://desktop.tauri.localhost:1420'
  || item.origin === 'http://localhost:1420'
  || item.origin === 'http://127.0.0.1:1420'
  || item.origin.startsWith('tauri://'))
const harnessContext = contexts.find(item =>
  item.origin.startsWith('http://dsh.tauri.localhost:')
  || item.origin.startsWith('http://127.0.0.1:'))
if (!appContext)
  throw new Error(`SURFACE_FIXTURE_CONTEXT_MISSING: ${JSON.stringify(contexts)}`)

async function evaluate(contextId, expression) {
  const answer = await call('Runtime.evaluate', {
    contextId,
    expression,
    awaitPromise: true,
    returnByValue: true,
  })
  if (answer.exceptionDetails)
    throw new Error(`SURFACE_FIXTURE_EVALUATION: ${answer.exceptionDetails.exception?.description || answer.exceptionDetails.text}`)
  return answer.result.value
}

const diagnostics = await evaluate(
  appContext.id,
  'globalThis.__TAURI_INTERNALS__.invoke("get_diagnostics_snapshot")',
)
if (diagnostics.coreCompatibility?.clientAbi !== 'split-client-v1') {
  console.log(`[surface-fixture] ${diagnostics.core.version}: 使用现有会话状态`)
  socket.close()
  process.exit()
}
if (!harnessContext)
  throw new Error(`SURFACE_FIXTURE_HARNESS_CONTEXT_MISSING: ${JSON.stringify(contexts)}`)

const fixturePath = resolve(import.meta.dirname, '..', 'target', 'surface-fixture')
await mkdir(fixturePath, { recursive: true })
const fixture = await evaluate(harnessContext.id, `(async () => {
  const rpc = async (method, args) => {
    const rpcId = crypto.randomUUID();
    const response = await fetch('/api/' + method, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ type: 'client-request', rpcId, method, payload: { args } }),
    });
    const envelope = await response.json();
    if (!envelope.result?.ok) throw new Error(method + ': ' + JSON.stringify(envelope.result?.error));
    return envelope.result.value;
  };
  const path = ${JSON.stringify(fixturePath)};
  const normalized = value => String(value).replaceAll('\\\\', '/').toLowerCase();
  const workspace = (await rpc('workspace/create', { request: { path } })).workspace;
  const sessions = (await rpc('session/list', { _request: {} })).items;
  let session = sessions.find(item => normalized(item.cwd) === normalized(path));
  if (!session) {
    const created = await rpc('session/create', { request: { workspaceId: workspace.workspaceId } });
    session = { sessionId: created.sessionId };
  }
  localStorage.setItem('dsh.sessions.current', JSON.stringify({ sessionId: session.sessionId }));
  location.reload();
  return { sessionId: session.sessionId };
})()`)
console.log(`[surface-fixture] ${diagnostics.core.version}: 会话 ${fixture.sessionId} 已选择`)
socket.close()
await new Promise(resolve => setTimeout(resolve, 1_500))
