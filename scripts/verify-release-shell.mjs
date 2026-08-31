import { spawn } from 'node:child_process'
import { readFile } from 'node:fs/promises'
import net from 'node:net'
import { resolve } from 'node:path'
import process from 'node:process'

const rawArgs = process.argv.slice(2)
const args = new Set(rawArgs)
const keepRunning = args.has('--keep-running')
const iterationsArgument = rawArgs.find(argument => argument.startsWith('--iterations='))
const iterations = Number.parseInt(iterationsArgument?.split('=')[1] ?? '3', 10)
if (!Number.isSafeInteger(iterations) || iterations < 1 || iterations > 20)
  throw new Error('--iterations must be an integer from 1 to 20')
const executableArg = rawArgs.find(argument => !argument.startsWith('--'))
const executable = resolve(executableArg ?? 'src-tauri/target/release/deepseek-harness-desktop.exe')
const endpointFile = process.env.DSH_DESKTOP_CONTROL_ENDPOINT_FILE
  ?? (process.platform === 'win32' && process.env.APPDATA
    ? `${process.env.APPDATA}\\io.github.hairyf.deepseek-harness-desktop\\control\\endpoint.json`
    : undefined)

if (!endpointFile)
  throw new Error('DSH_DESKTOP_CONTROL_ENDPOINT_FILE is required outside Windows')

function processExists(pid) {
  try {
    process.kill(pid, 0)
    return true
  }
  catch {
    return false
  }
}

async function readEndpoint() {
  try {
    return JSON.parse(await readFile(endpointFile, 'utf8'))
  }
  catch {
    return undefined
  }
}

async function invoke(endpoint, operation) {
  const request = `${JSON.stringify({
    token: endpoint.token,
    operation,
    args: {},
    traceId: `release-shell-${operation}-${Date.now()}`,
  })}\n`
  return await new Promise((resolve, reject) => {
    const socket = net.createConnection({ host: endpoint.host, port: endpoint.port })
    let response = ''
    socket.setEncoding('utf8')
    socket.setTimeout(10_000, () => socket.destroy(new Error(`${operation} timed out`)))
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

async function waitForEndpoint(pid) {
  const deadline = Date.now() + 15_000
  while (Date.now() < deadline) {
    const endpoint = await readEndpoint()
    if (endpoint?.pid === pid)
      return endpoint
    await new Promise(resolve => setTimeout(resolve, 100))
  }
  throw new Error(`Desktop control endpoint did not announce pid ${pid}`)
}

async function waitForShell(endpoint) {
  const deadline = Date.now() + 15_000
  while (Date.now() < deadline) {
    const status = await invoke(endpoint, 'shell.status')
    const events = status.runtime?.events ?? []
    const failure = events.find(event => ['resource-error', 'script-error', 'unhandled-rejection'].includes(event.stage))
    if (failure)
      throw new Error(`release shell ${failure.stage}: ${failure.resource || failure.message}`)
    if (status.assets.state === 'ready' && events.some(event => event.stage === 'react-mounted'))
      return status
    await new Promise(resolve => setTimeout(resolve, 100))
  }
  throw new Error('release shell did not mount React within 15 seconds')
}

async function waitForRuntime(endpoint) {
  const deadline = Date.now() + 45_000
  let lastError
  while (Date.now() < deadline) {
    try {
      await invoke(endpoint, 'runtime.health')
      const snapshot = await invoke(endpoint, 'diagnostics.snapshot')
      if (!snapshot.coreCompatibility?.coreVersion)
        throw new Error('active core compatibility is unavailable')
      if (snapshot.core?.version !== snapshot.coreCompatibility.coreVersion)
        throw new Error(`core version drift: ${snapshot.core?.version} != ${snapshot.coreCompatibility.coreVersion}`)
      if (snapshot.shellRuntime?.pid !== endpoint.pid)
        throw new Error(`shell diagnostics belong to pid ${snapshot.shellRuntime?.pid}, expected ${endpoint.pid}`)
      if (snapshot.webviewRoute?.state !== 'ready' || snapshot.webviewRoute.observedAtMs < endpoint.startedAtMs)
        throw new Error('current process did not publish a ready WebView route')
      if (snapshot.surface?.state !== 'ready' || snapshot.surface.observedAtMs < endpoint.startedAtMs)
        throw new Error('current process did not publish a ready surface report')
      return snapshot
    }
    catch (error) {
      lastError = error
    }
    await new Promise(resolve => setTimeout(resolve, 250))
  }
  throw new Error(`release runtime did not become ready: ${lastError}`)
}

async function waitForExit(pid) {
  const deadline = Date.now() + 15_000
  while (Date.now() < deadline) {
    if (!processExists(pid))
      return
    await new Promise(resolve => setTimeout(resolve, 100))
  }
  throw new Error(`Desktop pid ${pid} did not exit after shutdown acknowledgement`)
}

const existing = await readEndpoint()
if (existing?.pid && processExists(existing.pid))
  throw new Error(`Desktop is already running with pid ${existing.pid}; exit it before the release-shell gate`)

const cycles = []
for (let index = 0; index < iterations; index++) {
  const child = spawn(executable, [], { stdio: 'ignore', windowsHide: true })
  let endpoint
  try {
    endpoint = await waitForEndpoint(child.pid)
    const status = await waitForShell(endpoint)
    const snapshot = await waitForRuntime(endpoint)
    cycles.push({
      iteration: index + 1,
      pid: child.pid,
      coreVersion: snapshot.coreCompatibility.coreVersion,
      assetCount: status.assets.embeddedAssetCount,
      reactMounted: true,
      webviewReady: true,
      surfaceReady: true,
    })
  }
  finally {
    const preserveLast = keepRunning && index === iterations - 1
    if (preserveLast) {
      child.unref()
    }
    else if (endpoint) {
      try {
        await invoke(endpoint, 'desktop.shutdown')
        await waitForExit(child.pid)
      }
      catch (error) {
        child.kill()
        throw error
      }
    }
  }
}

console.log(JSON.stringify({
  protocol: 'dsh-desktop-release-shell-v2',
  iterations,
  cycles,
  failures: 0,
}, null, 2))
