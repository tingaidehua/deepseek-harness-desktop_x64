import { spawn } from 'node:child_process'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import net from 'node:net'
import { dirname, join } from 'node:path'
import process from 'node:process'
import { defineTool } from '@deepseek-ai/dsh-tools'

export const inject = ['tools']

const ENDPOINT_FILE = process.env.DSH_DESKTOP_CONTROL_ENDPOINT_FILE
const TRACE_FILE = process.env.DSH_DESKTOP_CONTROL_TRACE_FILE
const DESKTOP_EXECUTABLE = process.env.DSH_DESKTOP_EXECUTABLE
const HARNESS_PID_FILE = process.env.DSH_DESKTOP_HARNESS_PID_FILE
const RECOVERY_RECORD_FILE = ENDPOINT_FILE === undefined
  ? undefined
  : join(dirname(ENDPOINT_FILE), 'harness-recovery.json')
const READ_ONLY_OPERATIONS = [
  'control.catalog',
  'diagnostics.snapshot',
  'runtime.info',
  'runtime.health',
  'core.list',
  'profile.list',
  'logs.bundle',
  'trace.read',
]

function textOutput() {
  return {
    schema: { type: 'string' },
    render: (_args, value) => [{ type: 'text', text: value }],
  }
}

async function readEndpoint() {
  if (ENDPOINT_FILE === undefined)
    throw new Error('DESKTOP_CONTROL_NOT_CONFIGURED: endpoint file was not provided by Desktop')
  const endpoint = JSON.parse(await readFile(ENDPOINT_FILE, 'utf8'))
  if (endpoint.protocol !== 'dsh-desktop-control-jsonl-v1')
    throw new Error(`DESKTOP_CONTROL_PROTOCOL: unsupported protocol ${JSON.stringify(endpoint.protocol)}`)
  return endpoint
}

async function invokeDesktop(operation, args = {}) {
  const endpoint = await readEndpoint()
  const request = `${JSON.stringify({
    token: endpoint.token,
    operation,
    args,
    traceId: `dsh-${Date.now()}-${process.pid}`,
  })}\n`
  return await new Promise((resolve, reject) => {
    const socket = net.createConnection({ host: endpoint.host, port: endpoint.port })
    let response = ''
    const timeout = setTimeout(() => socket.destroy(new Error('DESKTOP_CONTROL_TIMEOUT')), 30_000)
    socket.setEncoding('utf8')
    socket.on('connect', () => socket.end(request))
    socket.on('data', chunk => response += chunk)
    socket.on('end', () => {
      clearTimeout(timeout)
      try {
        const parsed = JSON.parse(response)
        if (!parsed.ok)
          reject(new Error(parsed.error ?? 'DESKTOP_CONTROL_FAILED'))
        else resolve(parsed)
      }
      catch (error) {
        reject(error)
      }
    })
    socket.on('error', (error) => {
      clearTimeout(timeout)
      reject(error)
    })
  })
}

async function status() {
  try {
    const response = await invokeDesktop('runtime.info')
    return { desktop: 'running', endpointFile: ENDPOINT_FILE, runtime: response.result }
  }
  catch (error) {
    let traceTail = ''
    if (TRACE_FILE !== undefined) {
      try {
        const trace = await readFile(TRACE_FILE, 'utf8')
        traceTail = trace.slice(-64 * 1024)
      }
      catch {}
    }
    return {
      desktop: 'unavailable',
      endpointFile: ENDPOINT_FILE,
      error: error instanceof Error ? error.message : String(error),
      traceTail,
    }
  }
}

function restartDesktop(recoveryUrl) {
  if (DESKTOP_EXECUTABLE === undefined)
    throw new Error('DESKTOP_RECOVERY_NOT_CONFIGURED: executable was not provided by Desktop')
  const child = spawn(DESKTOP_EXECUTABLE, [], {
    detached: true,
    stdio: 'ignore',
    env: {
      ...process.env,
      DSH_DESKTOP_RECOVERY: '1',
      ...(recoveryUrl === undefined ? {} : { DSH_DESKTOP_RECOVERY_URL: recoveryUrl }),
    },
    windowsHide: true,
  })
  child.unref()
  return { started: true, pid: child.pid, executable: DESKTOP_EXECUTABLE }
}

function authenticatedUrlFrom(ctx) {
  const connection = typeof ctx.get === 'function' ? ctx.get('connection') : undefined
  const port = Number(process.env.DSH_WEB_PORT)
  return connection !== undefined
    && typeof connection.authenticatedUrl === 'function'
    && Number.isInteger(port)
    ? connection.authenticatedUrl(`http://127.0.0.1:${port}`)
    : undefined
}

async function publishRecoveryRecord(ctx) {
  let authenticatedUrl = authenticatedUrlFrom(ctx)
  const port = Number(process.env.DSH_WEB_PORT)
  if (RECOVERY_RECORD_FILE === undefined || HARNESS_PID_FILE === undefined || !Number.isInteger(port))
    return
  let identity
  try {
    const [pidLine, portLine] = (await readFile(HARNESS_PID_FILE, 'utf8')).split(/\r?\n/u)
    identity = { pid: Number(pidLine), port: Number(portLine) }
  }
  catch {
    return
  }
  if (!Number.isInteger(identity.pid) || identity.pid <= 0 || identity.port !== port)
    return
  if (authenticatedUrl === undefined) {
    try {
      const previous = JSON.parse(await readFile(RECOVERY_RECORD_FILE, 'utf8'))
      if (previous.protocol === 'dsh-desktop-harness-recovery-v1'
        && previous.pid === identity.pid
        && previous.port === identity.port
        && typeof previous.authenticatedUrl === 'string') {
        authenticatedUrl = previous.authenticatedUrl
      }
    }
    catch {}
  }
  await mkdir(dirname(RECOVERY_RECORD_FILE), { recursive: true })
  await writeFile(RECOVERY_RECORD_FILE, JSON.stringify({
    protocol: 'dsh-desktop-harness-recovery-v1',
    ...identity,
    ...(authenticatedUrl === undefined ? {} : { authenticatedUrl }),
  }), { mode: 0o600 })
}

async function recoveryUrlFrom(ctx) {
  const direct = authenticatedUrlFrom(ctx)
  if (direct !== undefined)
    return direct
  if (RECOVERY_RECORD_FILE === undefined)
    return undefined
  try {
    const record = JSON.parse(await readFile(RECOVERY_RECORD_FILE, 'utf8'))
    return record.protocol === 'dsh-desktop-harness-recovery-v1'
      && Number.isInteger(record.pid)
      && record.port === Number(process.env.DSH_WEB_PORT)
      && typeof record.authenticatedUrl === 'string'
      ? record.authenticatedUrl
      : undefined
  }
  catch {
    return undefined
  }
}

export async function apply(ctx) {
  await publishRecoveryRecord(ctx)
  if (typeof ctx.inject === 'function') {
    ctx.inject(['connection'], (connectionCtx) => {
      void publishRecoveryRecord(connectionCtx).catch((error) => {
        console.error(`dsh-desktop-control: failed to refresh recovery record: ${String(error)}`)
      })
    })
  }
  ctx.tools.register(defineTool({
    name: 'desktop_control_status',
    description: 'Inspect whether DeepSeek Harness Desktop is alive. When it is unavailable, return the latest persisted control trace so failures remain diagnosable without the GUI.',
    parameters: {},
    output: textOutput(),
    async execute() {
      return JSON.stringify(await status(), null, 2)
    },
  }))
  ctx.tools.register(defineTool({
    name: 'desktop_control_invoke',
    description: 'Call one allowlisted read-only Desktop diagnostic operation through its authenticated loopback control plane.',
    parameters: {
      operation: { type: 'string', required: true, enum: READ_ONLY_OPERATIONS },
      args: { type: 'json', description: 'Optional JSON arguments for the selected operation.' },
    },
    output: textOutput(),
    async execute(args) {
      return JSON.stringify(await invokeDesktop(args.operation, args.args ?? {}), null, 2)
    },
  }))
  ctx.tools.register(defineTool({
    name: 'desktop_control_stress',
    description: 'Run the Desktop diagnostics snapshot concurrently and return latency and failure statistics without operating the GUI.',
    parameters: {
      iterations: { type: 'integer', description: 'Number of snapshots, from 1 to 1000.' },
      concurrency: { type: 'integer', description: 'Concurrent snapshots, from 1 to 64.' },
    },
    output: textOutput(),
    async execute(args) {
      return JSON.stringify(await invokeDesktop('stress.snapshot', args), null, 2)
    },
  }))
  ctx.tools.register(defineTool({
    name: 'desktop_control_recover',
    description: 'Start a new Desktop shell after confirming it is unavailable. The current DSH process remains alive and is adopted by the recovered shell.',
    parameters: {},
    output: textOutput(),
    async execute() {
      const current = await status()
      if (current.desktop === 'running')
        return JSON.stringify({ started: false, reason: 'desktop is already running', current }, null, 2)
      const recoveryUrl = await recoveryUrlFrom(ctx)
      return JSON.stringify({ ...restartDesktop(recoveryUrl), previous: current }, null, 2)
    },
  }))
}
