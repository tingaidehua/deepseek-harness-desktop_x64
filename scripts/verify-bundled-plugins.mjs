import { existsSync, readFileSync } from 'node:fs'
import { join, resolve } from 'node:path'
import process from 'node:process'

const root = resolve(import.meta.dirname, '..')
const resources = join(root, 'src-tauri', 'resources')
const internal = JSON.parse(readFileSync(join(resources, 'internal-plugins.json'), 'utf8'))
const presets = JSON.parse(readFileSync(join(resources, 'preset-plugins.json'), 'utf8'))
const targets = JSON.parse(readFileSync(join(resources, 'core-compatibility.json'), 'utf8'))
const requested = new Set((process.env.DSH_DESKTOP_BUNDLE_PLUGIN_IDS || '').split(',').map(value => value.trim()).filter(Boolean))
const selected = plugins => requested.size === 0 ? plugins : plugins.filter(plugin => requested.has(plugin.id))
const failures = []

function requireFile(path, label) {
  if (!existsSync(path))
    failures.push(`${label}: ${path}`)
}

for (const target of targets) {
  for (const plugin of selected(internal)) {
    const pluginRoot = join(resources, 'internal-plugins', target.pluginArtifactSet, plugin.id)
    requireFile(join(pluginRoot, 'package.json'), `${target.coreVersion}/${plugin.id} 缺少包清单`)
    if (plugin.id === 'dsh-tauri-worktree') {
      requireFile(join(pluginRoot, 'dist', 'index.js'), `${target.coreVersion}/${plugin.id} 缺少 Host 入口`)
      requireFile(join(pluginRoot, 'dist', 'client.js'), `${target.coreVersion}/${plugin.id} 缺少 Client 入口`)
    }
  }
  for (const plugin of selected(presets)) {
    if (plugin.id === 'dsh-win-terminal-inspector' && target.providesWinTerminalInspector)
      continue
    requireFile(
      join(resources, 'preset-plugin-artifacts', target.pluginArtifactSet, plugin.id, 'package.json'),
      `${target.coreVersion}/${plugin.id} 缺少首次引导制品`,
    )
  }
}

if (failures.length > 0) {
  console.error(`[bundled-plugins] 验证失败\n${failures.map(value => `- ${value}`).join('\n')}`)
  process.exit(1)
}
console.log(`[bundled-plugins] ${targets.length} 个核心版本的 Desktop 插件制品完整`)
