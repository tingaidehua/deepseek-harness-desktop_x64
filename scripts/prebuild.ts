/**
 * Desktop 扩展预打包：默认把
 * 内置插件和社区预设清单制备为按已验证 DSH 版本区分的随包产物，分别写入
 * `resources/internal-plugins/dsh-v<version>/<id>` 与
 * `resources/preset-plugin-artifacts/dsh-v<version>/<id>`。
 * （随 `bundle.resources` 随安装包分发）。两种来源：
 *
 * - `github:owner/repo`：从上游仓库克隆、安装依赖并构建（源码形态的插件）；
 * - npm 包名（`name[@version]`）：从 npm registry 拉取已发布产物，跳过构建
 *   （发布包自带 lib/，如 dsh-tauri@0.2.0）。
 *
 * `pnpm build` 会调用本脚本。正式构建默认携带 Desktop 插件的版本化制品；
 * 只有显式设置 `DSH_DESKTOP_BUNDLE_EXTENSIONS=0` 的官方 DSH 基线测试才会清理它们。
 * 预打包只生成安装包资源，不会在构建机的 profile 中安装或启用插件。
 *
 * 约束：仅用 Node 内置模块（零新增依赖）；需要 git 与 pnpm 在 PATH 上；
 * 构建机器需可访问 GitHub 与 npm registry。通过 `tsx scripts/prebuild.ts`
 * 直接运行（TS + ESM），无需预编译。
 */
import { spawnSync } from 'node:child_process'
import {
  cpSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import process from 'node:process'

interface InternalPlugin {
  id: string
  spec: string
}

interface CoreTarget {
  coreVersion: string
  pluginArtifactSet: string
}

interface PluginSet {
  coreVersion: string
  artifactSet: string
  internalPlugins: InternalPlugin[]
  presetPlugins: InternalPlugin[]
  adapter: string
}

const REPO_ROOT = resolve(import.meta.dirname, '..')
const INTERNAL_PLUGINS_FILE = join(REPO_ROOT, 'src-tauri', 'resources', 'internal-plugins.json')
const PRESET_PLUGINS_FILE = join(REPO_ROOT, 'src-tauri', 'resources', 'preset-plugins.json')
const CORE_COMPATIBILITY_FILE = join(REPO_ROOT, 'src-tauri', 'resources', 'core-compatibility.json')
const BUNDLE_ROOT = join(REPO_ROOT, 'src-tauri', 'resources', 'internal-plugins')
const PRESET_BUNDLE_ROOT = join(REPO_ROOT, 'src-tauri', 'resources', 'preset-plugin-artifacts')
const GIT_URL_RE = /^github:([^#/]+\/[^#/]+)(?:#.*)?$/
const LOCAL_SOURCE_RE = /^local:(.+)$/
const CORE_TARGETS = JSON.parse(readFileSync(CORE_COMPATIBILITY_FILE, 'utf8')) as CoreTarget[]
const PLUGIN_SETS = CORE_TARGETS.map((target) => {
  const directory = join(REPO_ROOT, 'plugins', target.pluginArtifactSet)
  const set = JSON.parse(readFileSync(join(directory, 'plugin-set.json'), 'utf8')) as PluginSet
  if (set.coreVersion !== target.coreVersion || set.artifactSet !== target.pluginArtifactSet)
    die(`${directory}: plugin set identity does not match core compatibility record`)
  return { ...set, directory }
})

function die(message: string): never {
  console.error(`[prebuild] ${message}`)
  process.exit(1)
}

/** 同步执行命令，非零退出码即终止构建（内置插件缺失是发布缺陷，必须响亮失败）。 */
function run(
  program: string,
  args: readonly string[],
  cwd: string,
  shell = process.platform === 'win32',
): void {
  console.log(`[prebuild] $ ${program} ${args.join(' ')}`)
  const result = spawnSync(program, [...args], {
    cwd,
    stdio: 'inherit',
    shell,
  })
  if (result.error !== undefined) {
    die(`${program} 启动失败: ${result.error.message}`)
  }
  if (result.status !== 0) {
    die(`${program} ${args.join(' ')} 退出码 ${result.status}`)
  }
}

/** `github:owner/repo[#ref]` → 可克隆的 https URL（忽略 ref，拉默认分支最新）。 */
function githubSource(spec: string): { url: string, revision?: string } {
  const match = GIT_URL_RE.exec(spec)
  if (match === null) {
    die(`internal 插件 spec 必须是 github:owner/repo 形式，当前为: ${spec}`)
  }
  const repo = match[1].replace(/\.git$/, '')
  const hash = spec.indexOf('#')
  return {
    url: `https://github.com/${repo}.git`,
    revision: hash === -1 ? undefined : spec.slice(hash + 1),
  }
}

/** `name[@version]`（含 scoped `@scope/name[@version]`）→ 裸包名，用于定位 node_modules。 */
function npmPackageName(spec: string): string {
  const at = spec.indexOf('@', spec.startsWith('@') ? spec.indexOf('/') + 1 : 0)
  return at === -1 ? spec : spec.slice(0, at)
}

/**
 * 从 npm registry 拉取已发布产物：临时工程里 `pnpm add <spec>`，产物即
 * `node_modules/<name>/`（发布包自带 lib/ 等运行必需文件，无需再构建）。
 * 依赖 pnpm 在 PATH 上（与 git 来源流程同一前提）。
 */
function fetchNpmPackage(preset: InternalPlugin, temp: string): string {
  const project = join(temp, 'project')
  mkdirSync(project, { recursive: true })
  writeFileSync(join(project, 'package.json'), JSON.stringify({ private: true }))
  run('pnpm', ['add', preset.spec, '--ignore-scripts'], project)
  const pkgDir = join(project, 'node_modules', npmPackageName(preset.spec))
  if (!existsSync(join(pkgDir, 'package.json'))) {
    die(`${preset.id}: npm 安装后未找到产物 ${pkgDir}`)
  }
  console.log(`[prebuild] ${preset.id}: 来源 npm ${preset.spec}`)
  return pkgDir
}

/**
 * 拷贝构建产物：优先 `files` 白名单（只发运行必需：lib/、patch 文件、README），
 * 缺失白名单时拷贝整目录但排除 node_modules/.git 等开发噪声；
 * `package.json` 恒在（它是 `pnpm add file:<dir>` 的包名/入口来源）。
 */
function collectBundle(preset: InternalPlugin, clone: string, artifactSet: string, root = BUNDLE_ROOT): string {
  const dest = join(root, artifactSet, preset.id)
  mkdirSync(dest, { recursive: true })

  const manifest = JSON.parse(readFileSync(join(clone, 'package.json'), 'utf8')) as Record<string, unknown>
  const rawFiles = manifest.files
  const files = Array.isArray(rawFiles)
    ? rawFiles.filter((f): f is string => typeof f === 'string')
    : undefined
  const skip = new Set(['node_modules', '.git', '.gitignore', '.npmrc'])
  const hasGlob = files?.some(name => /[*?[\]{}]/.test(name)) ?? false
  const entries = files !== undefined && files.length > 0 && !hasGlob
    ? files
    : readdirSync(clone).filter(name => !skip.has(name) && !name.endsWith('.tsbuildinfo'))

  for (const name of entries) {
    const src = join(clone, name)
    if (!existsSync(src)) {
      die(`${preset.id}: 白名单产物缺失 ${src}`)
    }
    cpSync(src, join(dest, name), { recursive: true })
  }
  // 拷贝后置，确保即使白名单里没有 package.json 它也一定存在
  cpSync(join(clone, 'package.json'), join(dest, 'package.json'))
  return dest
}

/** 构建单个精确 DSH tag 的插件；版本差异只允许存在于该 tag 的插件目录。 */
function buildPlugin(
  preset: InternalPlugin,
  target: PluginSet & { directory: string },
  root = BUNDLE_ROOT,
): void {
  rmSync(join(root, target.artifactSet, preset.id), { recursive: true, force: true })

  const temp = mkdtempSync(join(tmpdir(), `dsh-internal-${preset.id}-`))
  let source: string
  const localMatch = LOCAL_SOURCE_RE.exec(preset.spec)
  if (localMatch !== null) {
    source = resolve(REPO_ROOT, localMatch[1])
    if (!existsSync(join(source, 'package.json')))
      die(`${preset.id}: 本地插件源缺少 package.json: ${source}`)
    console.log(`[prebuild] ${preset.id}: 来源仓库内 ${localMatch[1]}`)
  }
  else if (preset.spec.startsWith('github:')) {
    const clone = join(temp, preset.id)
    const gitSource = githubSource(preset.spec)
    run('git', ['clone', '--quiet', gitSource.url, clone], temp)
    if (gitSource.revision !== undefined)
      run('git', ['checkout', '--quiet', gitSource.revision], clone)

    const revision = spawnSync('git', ['-C', clone, 'rev-parse', '--short', 'HEAD'], { encoding: 'utf8' })
    if (revision.status === 0) {
      console.log(`[prebuild] ${preset.id}: 来源修订 ${revision.stdout.trim()}`)
    }

    // 注意：pnpm ≥10 默认拦截依赖的构建脚本（esbuild/原生模块需在插件仓库
    // 的 pnpm-workspace.yaml 配 onlyBuiltDependencies 放行）；纯 JS/TS 插件不受影响。
    run('pnpm', ['install'], clone)
    const manifest = JSON.parse(readFileSync(join(clone, 'package.json'), 'utf8')) as {
      scripts?: Record<string, string>
    }
    if (manifest.scripts?.build !== undefined) {
      run('pnpm', ['run', 'build'], clone)
    }
    source = clone
  }
  else {
    source = fetchNpmPackage(preset, temp)
  }

  const dest = collectBundle(preset, source, target.artifactSet, root)
  run(process.execPath, [resolve(target.directory, target.adapter), dest], REPO_ROOT, false)
  rmSync(temp, { recursive: true, force: true })
  console.log(`[prebuild] ${target.coreVersion}/${preset.id}: 精确版本产物已就绪`)
}

/** 删除不再受支持的旧 DSH tag 制品目录，只保留兼容表声明的两个精确集合。 */
function removeUnsupportedArtifactSets(root: string): void {
  if (!existsSync(root)) return
  const retained = new Set(PLUGIN_SETS.map(set => set.artifactSet))
  for (const entry of readdirSync(root, { withFileTypes: true })) {
    if (entry.isDirectory() && !retained.has(entry.name))
      rmSync(join(root, entry.name), { recursive: true, force: true })
  }
}

function main(): void {
  const bundleExtensions = process.env.DSH_DESKTOP_BUNDLE_EXTENSIONS
  if (bundleExtensions !== undefined && bundleExtensions !== '0' && bundleExtensions !== '1')
    die(`DSH_DESKTOP_BUNDLE_EXTENSIONS 只能是 0 或 1，当前为: ${bundleExtensions}`)
  if (bundleExtensions === '0') {
    rmSync(BUNDLE_ROOT, { recursive: true, force: true })
    rmSync(PRESET_BUNDLE_ROOT, { recursive: true, force: true })
    console.log('[prebuild] Desktop extensions explicitly disabled; building the clean official DSH baseline')
    return
  }
  if (!existsSync(INTERNAL_PLUGINS_FILE)) {
    die(`未找到内部插件清单 ${INTERNAL_PLUGINS_FILE}`)
  }
  if (!existsSync(PRESET_PLUGINS_FILE)) {
    die(`未找到首次引导插件清单 ${PRESET_PLUGINS_FILE}`)
  }
  removeUnsupportedArtifactSets(BUNDLE_ROOT)
  removeUnsupportedArtifactSets(PRESET_BUNDLE_ROOT)
  const requestedIds = new Set(
    (process.env.DSH_DESKTOP_BUNDLE_PLUGIN_IDS ?? '')
      .split(',')
      .map(id => id.trim())
      .filter(Boolean),
  )
  const allPlugins = PLUGIN_SETS.flatMap(set => [...set.internalPlugins, ...set.presetPlugins])
  const unknownIds = [...requestedIds].filter(id =>
    !allPlugins.some(plugin => plugin.id === id),
  )
  if (unknownIds.length > 0)
    die(`指定了未知插件: ${unknownIds.join(', ')}`)
  for (const target of PLUGIN_SETS) {
    const selectedInternal = target.internalPlugins.filter(plugin => requestedIds.size === 0 || requestedIds.has(plugin.id))
    const selectedPresets = target.presetPlugins.filter(plugin => requestedIds.size === 0 || requestedIds.has(plugin.id))
    console.log(`[prebuild] ${target.coreVersion}: 制备 ${selectedInternal.length} 个 internal 插件`)
    for (const plugin of selectedInternal)
      buildPlugin(plugin, target)
    console.log(`[prebuild] ${target.coreVersion}: 制备 ${selectedPresets.length} 个社区预设插件`)
    for (const plugin of selectedPresets)
      buildPlugin(plugin, target, PRESET_BUNDLE_ROOT)
  }
  console.log(`[prebuild] 完成 → ${BUNDLE_ROOT}, ${PRESET_BUNDLE_ROOT}`)
}

main()
