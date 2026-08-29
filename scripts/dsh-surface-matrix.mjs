import { readFile } from 'node:fs/promises'
import { dirname, resolve } from 'node:path'
import process from 'node:process'

const cdpPort = process.env.DSH_DESKTOP_CDP_PORT || '9337'
const contracts = JSON.parse(await readFile(new URL('../src-tauri/resources/surface-contracts.json', import.meta.url), 'utf8'))
const targets = await (await fetch(`http://127.0.0.1:${cdpPort}/json/list`)).json()
const appTarget = targets.find(item => item.type === 'page' && (
  item.url === 'http://tauri.localhost/'
  || item.url === 'http://desktop.tauri.localhost:1420/'
  || item.url === 'http://localhost:1420/'
  || item.url === 'http://127.0.0.1:1420/'
))
if (!appTarget)
  throw new Error('SURFACE_MATRIX_APP_TARGET_MISSING: start Desktop with WebView2 remote debugging enabled')
const target = targets.find(item => item.url.startsWith('http://127.0.0.1:') || item.url.startsWith('http://dsh.tauri.localhost:'))
  || targets.find(item => item.type === 'page')
if (!target)
  throw new Error('SURFACE_MATRIX_TARGET_MISSING: start Desktop with WebView2 remote debugging enabled')

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
await new Promise(resolve => setTimeout(resolve, 250))
const context = contexts.find(item =>
  item.origin.startsWith('http://dsh.tauri.localhost:') || item.origin.startsWith('http://127.0.0.1:'))
if (!context)
  throw new Error(`SURFACE_MATRIX_CONTEXT_MISSING: ${JSON.stringify(contexts)}`)

async function evaluateIn(contextId, expression) {
  const answer = await call('Runtime.evaluate', {
    contextId,
    expression,
    awaitPromise: true,
    returnByValue: true,
  })
  if (answer.exceptionDetails)
    throw new Error(`SURFACE_MATRIX_EVALUATION: ${answer.exceptionDetails.text}`)
  return answer.result.value
}

async function evaluate(expression) {
  return evaluateIn(context.id, expression)
}

async function evaluateApp(expression) {
  const appSocket = new WebSocket(appTarget.webSocketDebuggerUrl)
  const appPending = new Map()
  const appContexts = []
  let appSequence = 0
  appSocket.addEventListener('message', (event) => {
    const message = JSON.parse(event.data)
    if (message.method === 'Runtime.executionContextCreated')
      appContexts.push(message.params.context)
    if (!message.id || !appPending.has(message.id))
      return
    const operation = appPending.get(message.id)
    appPending.delete(message.id)
    if (message.error)
      operation.reject(new Error(JSON.stringify(message.error)))
    else operation.resolve(message.result)
  })
  await new Promise((resolve, reject) => {
    appSocket.addEventListener('open', resolve, { once: true })
    appSocket.addEventListener('error', reject, { once: true })
  })
  function appCall(method, params = {}) {
    const id = ++appSequence
    appSocket.send(JSON.stringify({ id, method, params }))
    return new Promise((resolve, reject) => appPending.set(id, { resolve, reject }))
  }
  await appCall('Runtime.enable')
  await new Promise(resolve => setTimeout(resolve, 100))
  const appContext = appContexts.find(item =>
    item.origin === 'http://tauri.localhost'
    || item.origin === 'http://desktop.tauri.localhost:1420'
    || item.origin === 'http://localhost:1420'
    || item.origin === 'http://127.0.0.1:1420'
    || item.origin.startsWith('tauri://'))
  if (!appContext)
    throw new Error(`SURFACE_MATRIX_APP_CONTEXT_MISSING: ${JSON.stringify(appContexts)}`)
  const answer = await appCall('Runtime.evaluate', {
    contextId: appContext.id,
    expression,
    awaitPromise: true,
    returnByValue: true,
  })
  appSocket.close()
  if (answer.exceptionDetails)
    throw new Error(`SURFACE_MATRIX_APP_EVALUATION: ${answer.exceptionDetails.text}`)
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

for (const labels of [
  ['继续', 'Continue'],
  ['稍后配置', 'Configure later'],
]) {
  if (await click(labels))
    await new Promise(resolve => setTimeout(resolve, 250))
}

const observedLaunchProtocol = context.origin.startsWith('http://dsh.tauri.localhost:')
  ? 'token-cookie-v1'
  : 'direct-loopback-v1'
const diagnostics = await evaluateApp('globalThis.__TAURI_INTERNALS__.invoke("get_diagnostics_snapshot")')
const coreCompatibility = diagnostics.coreCompatibility
if (!coreCompatibility)
  throw new Error(`SURFACE_MATRIX_UNSUPPORTED_CORE: ${diagnostics.core.version}`)
if (coreCompatibility.webLaunchProtocol !== observedLaunchProtocol) {
  throw new Error(
    `SURFACE_MATRIX_PROTOCOL_MISMATCH: core=${coreCompatibility.coreVersion}; expected=${coreCompatibility.webLaunchProtocol}; observed=${observedLaunchProtocol}`,
  )
}
const checks = []
const warnings = []
function record(id, ok, detail) {
  checks.push({ id, ok, detail })
}

const cores = await evaluateApp('globalThis.__TAURI_INTERNALS__.invoke("get_cores")')
for (const version of contracts.coreVersions) {
  const rows = cores.filter(core => core.source === 'app' && core.version === version)
  record(
    `core.catalog.${version}`,
    rows.length === 1 && rows[0].present,
    `rows=${rows.length}; present=${rows[0]?.present === true}`,
  )
}
const activeCores = cores.filter(core => core.active)
record(
  'core.catalog.active-identity',
  activeCores.length === 1 && activeCores[0].version === diagnostics.core.version,
  `activeRows=${activeCores.length}; listed=${activeCores[0]?.version}; runtime=${diagnostics.core.version}`,
)

const bootstrap = await evaluate('({ loader: globalThis.__ModuleLoader__?.mode, ownsHost: globalThis.__DSH_TRANSPORT__?.ownsHost === true })')
record('web.loader-live', bootstrap.loader === 'live', `mode=${bootstrap.loader}`)
record(
  'web.host-privilege',
  coreCompatibility.webLaunchProtocol === 'direct-loopback-v1' || bootstrap.ownsHost,
  `webLaunchProtocol=${coreCompatibility.webLaunchProtocol}; ownsHost=${bootstrap.ownsHost}`,
)
const bootGraph = await evaluate('JSON.stringify(globalThis.__DSH_BOOT__ || {})')
for (const plugin of contracts.plugins) {
  record(`boot.plugin.${plugin}`, bootGraph.includes(`"${plugin}"`), 'declared in the composed Client boot graph')
}
for (const plugin of contracts.hostPlugins || []) {
  const appData = resolve(diagnostics.corePath, '..', '..', '..')
  const dshHome = resolve(dirname(diagnostics.profilePath), '..')
  let recovery
  let harnessPid
  try {
    recovery = JSON.parse(await readFile(resolve(appData, 'control', 'harness-recovery.json'), 'utf8'))
    harnessPid = Number((await readFile(resolve(dshHome, '.harness.pid'), 'utf8')).split(/\r?\n/)[0])
  }
  catch {}
  const declared = diagnostics.pluginCompatibility.checkedPackages.includes(plugin.id)
  const live = recovery?.protocol === plugin.probe
    && Number.isInteger(recovery.pid)
    && recovery.pid === harnessPid
  record(
    `host.plugin.${plugin.id}`,
    declared && diagnostics.pluginCompatibility.compatible && live,
    `declared=${declared}; compatible=${diagnostics.pluginCompatibility.compatible}; live=${live}`,
  )
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
  record(`settings.${surface.id}`, opened && expected && !failure, !opened ? 'navigation entry missing' : failure ? `visible failure: ${failure}` : `expected content=${expected}`)
  if (surface.id === 'plugins')
    pluginText = text
}

if (coreCompatibility.clientAbi === 'split-client-v1') {
  record('settings.plugins.subagent-card', pluginText.includes('Subagent'), 'alpha client capability')
}
record('settings.plugins.shell-card', pluginText.includes('终端') || pluginText.includes('Terminal'), 'Host settings namespace')

await click(['返回应用', 'Back to app'])
await new Promise(resolve => setTimeout(resolve, 250))
const worktree = await evaluate(`(() => {
  const anchor = document.querySelector(${JSON.stringify(contracts.worktree.modeAnchor)});
  const trigger = document.querySelector(${JSON.stringify(contracts.worktree.modeTrigger)});
  const sessionId = anchor?.getAttribute('data-dsh-tauri-worktree-mode-anchor') || '';
  const text = String(trigger?.innerText || trigger?.getAttribute('aria-label') || '');
  return { present: Boolean(anchor && trigger), sessionId, text };
})()`)
record(
  'worktree.mode-selector',
  worktree.present && contracts.worktree.localLabels.some(label => worktree.text.includes(label)),
  `present=${worktree.present}; session=${worktree.sessionId || 'missing'}; text=${worktree.text}`,
)
if (worktree.sessionId) {
  const status = await evaluate(`fetch(${JSON.stringify(contracts.worktree.statusPath)} + '?sessionId=' + encodeURIComponent(${JSON.stringify(worktree.sessionId)}), {
    credentials: 'same-origin', cache: 'no-store'
  }).then(async response => ({ status: response.status, body: await response.json() }))
    .catch(error => ({ status: 0, body: { error: String(error) } }))`)
  record(
    'worktree.status-api',
    status.status === 200 && ['local', 'worktree'].includes(status.body?.mode),
    `status=${status.status}; mode=${status.body?.mode || 'missing'}; error=${status.body?.error || ''}`,
  )
}
else {
  record('worktree.status-api', false, 'mode selector did not expose a session id')
}
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
  record(`sidebar.${tab.id}`, opened && !failure, !opened ? 'tab missing' : failure ? `visible failure: ${failure}` : 'opened without a known failure')
}

const finalText = await bodyText()
if (/SessionPersistenceCorruptionError|历史加载失败/.test(finalText)) {
  warnings.push('The selected core rejected persisted session data written by another prerelease core; the shell stayed responsive.')
}
const failed = checks.filter(check => !check.ok)
const report = {
  state: failed.length === 0 ? 'ready' : 'failed',
  coreCompatibility,
  origin: context.origin,
  checks,
  warnings,
}
console.log(JSON.stringify(report, null, 2))
socket.close()
if (failed.length > 0)
  process.exitCode = 1
