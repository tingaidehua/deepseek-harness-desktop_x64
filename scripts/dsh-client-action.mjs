import { readFile } from 'node:fs/promises'
import process from 'node:process'

const action = process.argv[2]
const supportedActions = new Set([
  'session.click-new',
  'session.start-unscoped',
  'session.start-current-workspace',
  'session.connect-current-workspace',
  'session.open-current-workspace-blank',
  'session.open-nonblank',
  'session.click-archive',
])
if (!supportedActions.has(action))
  throw new Error(`CLIENT_ACTION_UNSUPPORTED: ${String(action)}`)
if (process.platform !== 'win32' || !process.env.APPDATA)
  throw new Error('CLIENT_ACTION_APP_DATA_REQUIRED')

const recordPath = `${process.env.APPDATA}\\io.github.hairyf.deepseek-harness-desktop\\control\\harness-recovery.json`
const recovery = JSON.parse(await readFile(recordPath, 'utf8'))
if (!Number.isInteger(recovery.port))
  throw new Error('CLIENT_ACTION_PORT_MISSING')
let cookie = ''
if (typeof recovery.authenticatedUrl === 'string') {
  const exchange = await fetch(recovery.authenticatedUrl, { redirect: 'manual' })
  cookie = exchange.headers.getSetCookie().map(value => value.split(';', 1)[0]).join('; ')
  if (![302, 303].includes(exchange.status) || cookie.length === 0)
    throw new Error(`CLIENT_ACTION_AUTH_EXCHANGE_FAILED: status=${exchange.status}`)
}
const base = `http://127.0.0.1:${recovery.port}/api/dsh-desktop-control`
const headers = {
  'content-type': 'application/json',
  ...(cookie === '' ? {} : { cookie }),
}
const id = `client-action-${Date.now()}-${process.pid}`
const accepted = await fetch(`${base}/action`, {
  method: 'POST',
  headers,
  body: JSON.stringify({ id, action }),
})
if (accepted.status !== 202)
  throw new Error(`CLIENT_ACTION_REJECTED: status=${accepted.status}; body=${await accepted.text()}`)

const deadline = Date.now() + 15_000
let latest
while (Date.now() < deadline) {
  const response = await fetch(`${base}/state`, { headers, cache: 'no-store' })
  if (!response.ok)
    throw new Error(`CLIENT_ACTION_STATE_FAILED: status=${response.status}`)
  latest = (await response.json()).report
  if (latest?.id === id && ['completed', 'failed'].includes(latest.phase))
    break
  await new Promise(resolve => setTimeout(resolve, 100))
}
if (latest?.id !== id)
  throw new Error('CLIENT_ACTION_REPORT_TIMEOUT')
if (latest.phase !== 'completed')
  throw new Error(`CLIENT_ACTION_FAILED: ${JSON.stringify(latest)}`)

const snapshot = latest.after ?? latest.archived ?? latest.before ?? {}
console.log(JSON.stringify({
  protocol: 'dsh-desktop-client-action-v1',
  action,
  phase: latest.phase,
  currentSessionId: snapshot.current ?? null,
  currentBlank: snapshot.currentBlank ?? false,
  archivedSessionIds: snapshot.archivedSessionIds ?? [],
  expectedSessionId: latest.expectedId ?? null,
  serviceCalls: latest.currentServiceCalls ?? null,
  rolledBack: latest.rolledBack ?? false,
  observedAtMs: latest.observedAtMs,
  failures: 0,
}, null, 2))
