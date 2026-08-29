import { readFile } from 'node:fs/promises'

const cdpPort = process.env.DSH_DESKTOP_CDP_PORT || '9337'
const contracts = JSON.parse(await readFile(new URL('../src-tauri/resources/surface-contracts.json', import.meta.url), 'utf8'))
const targets = await (await fetch(`http://127.0.0.1:${cdpPort}/json/list`)).json()
const target = targets.find(item => item.url.startsWith('http://127.0.0.1:'))
  || targets.find(item => item.type === 'page')
if (!target) throw new Error('SURFACE_MATRIX_TARGET_MISSING: start Desktop with WebView2 remote debugging enabled')

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
const context = contexts.find(item =>
  item.origin.startsWith('http://dsh.tauri.localhost:') || item.origin.startsWith('http://127.0.0.1:'))
if (!context) throw new Error(`SURFACE_MATRIX_CONTEXT_MISSING: ${JSON.stringify(contexts)}`)

async function evaluate(expression) {
  const answer = await call('Runtime.evaluate', {
    contextId: context.id,
    expression,
    awaitPromise: true,
    returnByValue: true,
  })
  if (answer.exceptionDetails) throw new Error(`SURFACE_MATRIX_EVALUATION: ${answer.exceptionDetails.text}`)
  return answer.result.value
}

async function click(labels) {
  return evaluate(`(() => {
    const labels = ${JSON.stringify(labels)};
    const candidates = [...document.querySelectorAll('[title],button,a,[role="button"]')];
    const target = candidates.find(element => labels.includes((element.getAttribute('title') || '').trim()))
      || candidates.find(element => labels.includes((element.innerText || element.getAttribute('aria-label') || '').trim()));
    if (!target) return false;
    target.click();
    return true;
  })()`)
}

async function bodyText() {
  return evaluate('String(document.body && document.body.innerText || "")')
}

function visibleFailure(text) {
  const lower = text.toLowerCase()
  return contracts.visibleFailurePatterns.find(pattern => lower.includes(pattern.toLowerCase()))
}

const adapter = context.origin.startsWith('http://dsh.tauri.localhost:')
  ? 'authenticated-web-v1'
  : 'legacy-web'
const checks = []
const warnings = []
function record(id, ok, detail) {
  checks.push({ id, ok, detail })
}

const bootstrap = await evaluate('({ loader: globalThis.__ModuleLoader__?.mode, ownsHost: globalThis.__DSH_TRANSPORT__?.ownsHost === true })')
record('web.loader-live', bootstrap.loader === 'live', `mode=${bootstrap.loader}`)
record('web.host-privilege', adapter === 'legacy-web' || bootstrap.ownsHost, `adapter=${adapter}; ownsHost=${bootstrap.ownsHost}`)
const bootGraph = await evaluate('JSON.stringify(globalThis.__DSH_BOOT__ || {})')
for (const plugin of contracts.plugins) {
  record(`boot.plugin.${plugin}`, bootGraph.includes(`"${plugin}"`), 'declared in the composed Client boot graph')
}

for (const resource of contracts.resources) {
  const result = await evaluate(`fetch(${JSON.stringify(resource.path)}, { credentials: 'same-origin', cache: 'no-store' })
    .then(response => ({ ok: response.ok, status: response.status }))
    .catch(error => ({ ok: false, status: String(error) }))`)
  record(resource.id, result.ok, `${resource.path} -> ${result.status}`)
}

await click(['打开侧边栏', 'Open sidebar'])
const settingsAlreadyOpen = await evaluate(`
  [...document.querySelectorAll('button,a,[role="button"]')]
    .some(element => ['模型', 'Models'].includes((element.innerText || '').trim()))`)
record('settings.open', settingsAlreadyOpen || await click(['设置', 'Settings']), 'settings trigger')
await new Promise(resolve => setTimeout(resolve, 350))
let pluginText = ''
for (const surface of contracts.settingsSurfaces) {
  const opened = await click(surface.labels)
  await new Promise(resolve => setTimeout(resolve, 350))
  const text = await bodyText()
  const failure = visibleFailure(text)
  const expected = surface.expectedAny.some(value => text.includes(value))
  record(`settings.${surface.id}`, opened && expected && !failure,
    !opened ? 'navigation entry missing' : failure ? `visible failure: ${failure}` : `expected content=${expected}`)
  if (surface.id === 'plugins') pluginText = text
}

if (adapter === 'authenticated-web-v1') {
  record('settings.plugins.subagent-card', pluginText.includes('Subagent'), 'alpha client capability')
}
record('settings.plugins.shell-card', pluginText.includes('终端') || pluginText.includes('Terminal'), 'Host settings namespace')

await click(['返回应用', 'Back to app'])
await new Promise(resolve => setTimeout(resolve, 250))
for (const tab of contracts.sidebarTabs) {
  let opened = await click(tab.labels)
  if (!opened) {
    await click(['新建标签页', 'New tab'])
    await new Promise(resolve => setTimeout(resolve, 150))
    opened = await click(tab.labels)
  }
  await new Promise(resolve => setTimeout(resolve, 350))
  const text = await bodyText()
  const failure = visibleFailure(text)
  record(`sidebar.${tab.id}`, opened && !failure,
    !opened ? 'tab missing' : failure ? `visible failure: ${failure}` : 'opened without a known failure')
}

const finalText = await bodyText()
if (/SessionPersistenceCorruptionError|历史加载失败/.test(finalText)) {
  warnings.push('The selected core rejected persisted session data written by another prerelease core; the shell stayed responsive.')
}
const failed = checks.filter(check => !check.ok)
const report = {
  state: failed.length === 0 ? 'ready' : 'failed',
  adapter,
  origin: context.origin,
  checks,
  warnings,
}
console.log(JSON.stringify(report, null, 2))
socket.close()
if (failed.length > 0) process.exitCode = 1
