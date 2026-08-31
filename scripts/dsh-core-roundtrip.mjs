import { spawn } from 'node:child_process'
import { readFile } from 'node:fs/promises'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

const contracts = JSON.parse(await readFile(new URL('../src-tauri/resources/surface-contracts.json', import.meta.url), 'utf8'))
if (!Array.isArray(contracts.coreRoundTrip) || contracts.coreRoundTrip.length < 2)
  throw new Error('CORE_ROUNDTRIP_CONTRACT_MISSING: surface-contracts.json must declare at least two versions')

async function runScript(scriptName, args = []) {
  await new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [
      fileURLToPath(new URL(scriptName, import.meta.url)),
      ...args,
    ], {
      env: process.env,
      stdio: 'inherit',
      windowsHide: true,
    })
    child.once('error', reject)
    child.once('exit', (code, signal) => {
      if (code === 0)
        resolve()
      else reject(new Error(`CORE_ROUNDTRIP_STEP_FAILED: script=${scriptName}; args=${args.join(',')}; code=${code}; signal=${signal}`))
    })
  })
}

for (const version of contracts.coreRoundTrip) {
  await runScript('./dsh-select-core.mjs', [version])
  await runScript('./dsh-client-action.mjs', ['session.open-nonblank'])
  await runScript('./dsh-client-action.mjs', ['session.click-new'])
  await runScript('./dsh-client-action.mjs', ['session.open-nonblank'])
  await runScript('./dsh-client-action.mjs', ['session.click-archive'])
}

console.log(JSON.stringify({
  protocol: 'desktop-core-roundtrip-v1',
  sequence: contracts.coreRoundTrip,
  finalVersion: contracts.coreRoundTrip.at(-1),
  failures: 0,
}, null, 2))
