import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import process from 'node:process'

const root = resolve(import.meta.dirname, '..')
const builder = readFileSync(resolve(root, 'src-tauri/src/desktop/builder.rs'), 'utf8')
const control = readFileSync(resolve(root, 'src-tauri/src/service/control.rs'), 'utf8')
const contracts = JSON.parse(readFileSync(resolve(root, 'src-tauri/resources/feature-test-contracts.json'), 'utf8'))

function fail(message) {
  console.error(`[feature-test-contract] ${message}`)
  process.exitCode = 1
}

const handlerPrefix = 'tauri::generate_handler!['
const handlerStart = builder.indexOf(handlerPrefix)
const handlerEnd = builder.indexOf('\n    ]', handlerStart + handlerPrefix.length)
if (handlerStart === -1 || handlerEnd === -1) {
  fail('无法读取 Tauri command 注册表')
  process.exit()
}
const handler = builder.slice(handlerStart + handlerPrefix.length, handlerEnd)
const registered = [...handler.matchAll(/crate::(?:bridge|desktop::notification)::([a-z0-9_]+)/g)]
  .map(match => match[1])
const declared = contracts.features.flatMap(feature => feature.commands)
const duplicateCommands = declared.filter((command, index) => declared.indexOf(command) !== index)
const missingCommands = registered.filter(command => !declared.includes(command))
const staleCommands = declared.filter(command => !registered.includes(command))

const implementedOperations = [...control.matchAll(/id:\s*"([a-z.]+)"/g)].map(match => match[1])
const declaredOperations = [...new Set(contracts.features.flatMap(feature => feature.controlOperations))]
const missingOperations = declaredOperations.filter(operation => !implementedOperations.includes(operation))

if (contracts.protocol !== 'desktop-feature-test-contract-v1')
  fail(`未知规约版本 ${JSON.stringify(contracts.protocol)}`)
if (duplicateCommands.length > 0)
  fail(`command 重复归属: ${[...new Set(duplicateCommands)].join(', ')}`)
if (missingCommands.length > 0)
  fail(`新增或改名 command 未同步测试接口规约: ${missingCommands.join(', ')}`)
if (staleCommands.length > 0)
  fail(`已删除 command 仍残留在测试接口规约: ${staleCommands.join(', ')}`)
if (missingOperations.length > 0)
  fail(`规约引用了不存在的控制操作: ${missingOperations.join(', ')}`)
if (process.exitCode === undefined)
  console.log(`[feature-test-contract] ${registered.length} 个 command、${implementedOperations.length} 个外部控制操作已覆盖`)
