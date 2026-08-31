import { existsSync, readdirSync, readFileSync } from 'node:fs'
import { join, resolve } from 'node:path'
import process from 'node:process'

const root = resolve(import.meta.dirname, '..')
const resources = join(root, 'src-tauri', 'resources')
const targets = JSON.parse(readFileSync(join(resources, 'core-compatibility.json'), 'utf8'))
const requested = new Set((process.env.DSH_DESKTOP_BUNDLE_PLUGIN_IDS || '').split(',').map(value => value.trim()).filter(Boolean))
const selected = plugins => requested.size === 0 ? plugins : plugins.filter(plugin => requested.has(plugin.id))
const failures = []

function requireFile(path, label) {
  if (!existsSync(path))
    failures.push(`${label}: ${path}`)
}

for (const target of targets) {
  const setRoot = join(root, 'plugins', target.pluginArtifactSet)
  const pluginSet = JSON.parse(readFileSync(join(setRoot, 'plugin-set.json'), 'utf8'))
  if (pluginSet.coreVersion !== target.coreVersion || pluginSet.artifactSet !== target.pluginArtifactSet)
    failures.push(`${target.coreVersion}: 插件集合身份与兼容表不一致`)
  requireFile(join(setRoot, pluginSet.adapter), `${target.coreVersion}: 缺少版本专属制品适配入口`)
  const internal = pluginSet.internalPlugins
  const presets = pluginSet.presetPlugins
  for (const plugin of selected(internal)) {
    const pluginRoot = join(resources, 'internal-plugins', target.pluginArtifactSet, plugin.id)
    requireFile(join(pluginRoot, 'package.json'), `${target.coreVersion}/${plugin.id} 缺少包清单`)
    if (plugin.id === 'dsh-desktop-control') {
      const expectedSource = `local:plugins/${target.pluginArtifactSet}/dsh-desktop-control`
      if (plugin.spec !== expectedSource)
        failures.push(`${target.coreVersion}/${plugin.id}: 控制插件源码没有按精确 tag 隔离`)
      requireFile(join(pluginRoot, 'index.js'), `${target.coreVersion}/${plugin.id} 缺少 Host 入口`)
      const clientPath = join(pluginRoot, 'client.js')
      requireFile(clientPath, `${target.coreVersion}/${plugin.id} 缺少 Client 入口`)
      const manifestPath = join(pluginRoot, 'package.json')
      if (existsSync(manifestPath)) {
        const manifest = JSON.parse(readFileSync(manifestPath, 'utf8'))
        if (manifest.exports?.['./package.json'] !== './package.json')
          failures.push(`${target.coreVersion}/${plugin.id}: 未导出 package.json，DSH 客户端模块扫描器无法识别插件`)
      }
      if (existsSync(clientPath)) {
        const client = readFileSync(clientPath, 'utf8')
        const usesRcWorkspace = client.includes('ctx.workspaces.startSession')
        const usesAlphaWorkspace = client.includes("ctx.get('uiWorkspace').startSession")
        const retryLoop = client.indexOf('while (!stopped)')
        const readyReport = client.indexOf("await report({ phase: 'ready'")
        if (retryLoop === -1 || readyReport < retryLoop)
          failures.push(`${target.coreVersion}/${plugin.id}: Client ready 上报不在启动重试循环内`)
        if (!client.includes('window.__dshDesktopControlStop?.()')
          || !client.includes('window.__dshDesktopControlStop = () => { stopped = true }'))
          failures.push(`${target.coreVersion}/${plugin.id}: Client 控制循环没有页面级单例所有权`)
        if (target.workspaceNavigationProtocol === 'workspaces-service-v1' && (!usesRcWorkspace || usesAlphaWorkspace))
          failures.push(`${target.coreVersion}/${plugin.id}: 控制插件没有隔离到 RC2 workspaces 服务`)
        if (target.workspaceNavigationProtocol === 'ui-workspace-v1' && (usesRcWorkspace || !usesAlphaWorkspace))
          failures.push(`${target.coreVersion}/${plugin.id}: 控制插件没有隔离到 alpha uiWorkspace 服务`)
      }
    }
    if (plugin.id === 'dsh-tauri-worktree') {
      requireFile(join(pluginRoot, 'dist', 'index.js'), `${target.coreVersion}/${plugin.id} 缺少 Host 入口`)
      requireFile(join(pluginRoot, 'dist', 'client.js'), `${target.coreVersion}/${plugin.id} 缺少 Client 入口`)
    }
    if (plugin.id === 'dsh-tauri-panel') {
      const clientPath = join(pluginRoot, 'dist', 'client.js')
      requireFile(clientPath, `${target.coreVersion}/${plugin.id} 缺少 Client 入口`)
      if (existsSync(clientPath)) {
        const client = readFileSync(clientPath, 'utf8')
        const usesLegacyNavigation = client.includes('.workspaces.startSession(')
        const usesUiWorkspace = client.includes('.get(`uiWorkspace`).startSession(')
        if (target.workspaceNavigationProtocol === 'ui-workspace-v1' && (usesLegacyNavigation || !usesUiWorkspace))
          failures.push(`${target.coreVersion}/${plugin.id}: 未适配 uiWorkspace 新会话导航`)
        if (target.workspaceNavigationProtocol === 'workspaces-service-v1' && (!usesLegacyNavigation || usesUiWorkspace))
          failures.push(`${target.coreVersion}/${plugin.id}: 旧内核导航制品被错误改写`)
      }
    }
    if (plugin.id === 'dsh-tauri-session') {
      const clientPath = join(pluginRoot, 'dist', 'client.js')
      requireFile(clientPath, `${target.coreVersion}/${plugin.id} 缺少 Client 入口`)
      if (existsSync(clientPath)) {
        const client = readFileSync(clientPath, 'utf8')
        const legacyArchiveEffect = /,([A-Za-z_$][\w$]*)\.effect\(\(\)=>[A-Za-z_$][\w$]*\(\1\.workspaces,\1\.sessions\),[A-Za-z_$][\w$]*\)(?=\})/
        const active = legacyArchiveEffect.test(client)
        if (target.workspaceArchiveProtocol === 'core-native-v1' && active)
          failures.push(`${target.coreVersion}/${plugin.id}: 内核原生归档版本仍启用了 DOM/Fiber 归档补丁`)
        if (target.workspaceArchiveProtocol === 'plugin-workspace-patch-v1' && !active)
          failures.push(`${target.coreVersion}/${plugin.id}: 旧内核缺少插件归档补丁`)
      }
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

const retainedSets = new Set(targets.map(target => target.pluginArtifactSet))
for (const artifactRoot of ['internal-plugins', 'preset-plugin-artifacts']) {
  const path = join(resources, artifactRoot)
  if (!existsSync(path)) continue
  for (const entry of readdirSync(path, { withFileTypes: true })) {
    if (entry.isDirectory() && !retainedSets.has(entry.name))
      failures.push(`${artifactRoot}: 存在未支持的旧版本制品目录 ${entry.name}`)
  }
}

if (failures.length > 0) {
  console.error(`[bundled-plugins] 验证失败\n${failures.map(value => `- ${value}`).join('\n')}`)
  process.exit(1)
}
console.log(`[bundled-plugins] ${targets.length} 个内核版本的 Desktop 插件制品完整`)
