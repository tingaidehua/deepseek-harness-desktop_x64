import { readFile } from 'node:fs/promises'
import net from 'node:net'
import process from 'node:process'

const operation = process.argv[2]
if (!operation)
  throw new Error('CONTROL_OPERATION_REQUIRED')
const args = process.argv[3] === undefined ? {} : JSON.parse(process.argv[3])
const endpointFile = process.env.DSH_DESKTOP_CONTROL_ENDPOINT_FILE
  ?? (process.platform === 'win32' && process.env.APPDATA
    ? `${process.env.APPDATA}\\io.github.hairyf.deepseek-harness-desktop\\control\\endpoint.json`
    : undefined)
if (!endpointFile)
  throw new Error('DSH_DESKTOP_CONTROL_ENDPOINT_FILE_REQUIRED')
const endpoint = JSON.parse(await readFile(endpointFile, 'utf8'))
const request = `${JSON.stringify({
  token: endpoint.token,
  operation,
  args,
  traceId: `cli-${operation}-${Date.now()}-${process.pid}`,
})}\n`

const response = await new Promise((resolve, reject) => {
  const socket = net.createConnection({ host: endpoint.host, port: endpoint.port })
  let content = ''
  socket.setEncoding('utf8')
  socket.setTimeout(65_000, () => socket.destroy(new Error(`${operation} timed out`)))
  socket.on('connect', () => socket.end(request))
  socket.on('data', chunk => content += chunk)
  socket.on('end', () => {
    try {
      resolve(JSON.parse(content))
    }
    catch (error) {
      reject(error)
    }
  })
  socket.on('error', reject)
})
if (!response.ok)
  throw new Error(response.error ?? `${operation} failed`)
console.log(JSON.stringify({ protocol: 'dsh-desktop-control-cli-v1', operation, result: response.result }, null, 2))
