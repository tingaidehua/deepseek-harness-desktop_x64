import { spawn } from 'node:child_process'
import { readFile } from 'node:fs/promises'
import net from 'node:net'
import process from 'node:process'

const DEFAULT_ITERATIONS = 100
const DEFAULT_CONCURRENCY = 16

function boundedInteger(value, fallback, maximum, name) {
  const parsed = value === undefined ? fallback : Number(value)
  if (!Number.isInteger(parsed) || parsed < 1 || parsed > maximum)
    throw new Error(`${name} must be an integer between 1 and ${maximum}`)
  return parsed
}

function endpointPath() {
  if (process.env.DSH_DESKTOP_CONTROL_ENDPOINT_FILE !== undefined)
    return process.env.DSH_DESKTOP_CONTROL_ENDPOINT_FILE
  if (process.platform !== 'win32' || process.env.APPDATA === undefined)
    throw new Error('DSH_DESKTOP_CONTROL_ENDPOINT_FILE is required outside Windows')
  return `${process.env.APPDATA}\\io.github.hairyf.deepseek-harness-desktop\\control\\endpoint.json`
}

async function invoke(endpoint, operation) {
  const request = `${JSON.stringify({
    token: endpoint.token,
    operation,
    args: {},
    traceId: `single-instance-${Date.now()}-${process.pid}`,
  })}\n`
  return await new Promise((resolve, reject) => {
    const socket = net.createConnection({ host: endpoint.host, port: endpoint.port })
    let response = ''
    socket.setEncoding('utf8')
    socket.setTimeout(10_000, () => socket.destroy(new Error('single-instance control timeout')))
    socket.on('connect', () => socket.end(request))
    socket.on('data', chunk => response += chunk)
    socket.on('end', () => {
      try {
        const parsed = JSON.parse(response)
        if (!parsed.ok)
          reject(new Error(parsed.error ?? 'instance.info failed'))
        else resolve(parsed.result)
      }
      catch (error) {
        reject(error)
      }
    })
    socket.on('error', reject)
  })
}

async function launchProbe(executable) {
  const started = performance.now()
  await new Promise((resolve, reject) => {
    const child = spawn(executable, ['--single-instance-probe'], {
      stdio: 'ignore',
      windowsHide: true,
    })
    const timeout = setTimeout(() => {
      child.kill()
      reject(new Error(`secondary instance ${child.pid} did not exit within 10 seconds`))
    }, 10_000)
    child.once('error', (error) => {
      clearTimeout(timeout)
      reject(error)
    })
    child.once('exit', (code, signal) => {
      clearTimeout(timeout)
      if (code === 0)
        resolve()
      else reject(new Error(`secondary instance exited with code=${code} signal=${signal}`))
    })
  })
  return Math.round((performance.now() - started) * 1000)
}

const iterations = boundedInteger(
  process.env.DSH_DESKTOP_SINGLE_INSTANCE_ITERATIONS,
  DEFAULT_ITERATIONS,
  2_000,
  'DSH_DESKTOP_SINGLE_INSTANCE_ITERATIONS',
)
const concurrency = Math.min(iterations, boundedInteger(
  process.env.DSH_DESKTOP_SINGLE_INSTANCE_CONCURRENCY,
  DEFAULT_CONCURRENCY,
  64,
  'DSH_DESKTOP_SINGLE_INSTANCE_CONCURRENCY',
))
const path = endpointPath()
const beforeEndpoint = JSON.parse(await readFile(path, 'utf8'))
const before = await invoke(beforeEndpoint, 'instance.info')
const durations = []
let next = 0

await Promise.all(Array.from({ length: concurrency }, async () => {
  while (next < iterations) {
    next += 1
    durations.push(await launchProbe(before.executable))
  }
}))

const afterEndpoint = JSON.parse(await readFile(path, 'utf8'))
const after = await invoke(afterEndpoint, 'instance.info')
if (before.pid !== after.pid || beforeEndpoint.pid !== afterEndpoint.pid)
  throw new Error(`primary Desktop changed during stress: ${before.pid} -> ${after.pid}`)
durations.sort((left, right) => left - right)
function percentile(percent) {
  return durations[Math.min(
    durations.length - 1,
    Math.floor((durations.length - 1) * percent / 100),
  )]
}

console.log(JSON.stringify({
  protocol: 'desktop-single-instance-stress-v1',
  iterations,
  concurrency,
  primaryPid: after.pid,
  executable: after.executable,
  debug: after.debug,
  failures: 0,
  latencyMicros: {
    p50: percentile(50),
    p95: percentile(95),
    max: durations.at(-1),
  },
}, null, 2))
