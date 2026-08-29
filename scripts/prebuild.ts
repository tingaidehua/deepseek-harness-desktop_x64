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
  webLaunchProtocol: string
  clientAbi: string
  slotProtocol: string
  sessionFormat: string
  pluginArtifactSet: string
  supportsNoOpen: boolean
  providesWinTerminalInspector: boolean
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

function die(message: string): never {
  console.error(`[prebuild] ${message}`)
  process.exit(1)
}

/** 同步执行命令，非零退出码即终止构建（内置插件缺失是发布缺陷，必须响亮失败）。 */
function run(program: string, args: readonly string[], cwd: string): void {
  console.log(`[prebuild] $ ${program} ${args.join(' ')}`)
  const result = spawnSync(program, [...args], {
    cwd,
    stdio: 'inherit',
    shell: process.platform === 'win32',
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

/** Adapt the published RC client artifact to alpha.1's public client-store split. */
function adaptSplitClientV1(dest: string): void {
  const manifestPath = join(dest, 'package.json')
  const manifest = JSON.parse(readFileSync(manifestPath, 'utf8')) as {
    dependencies?: Record<string, string>
    peerDependencies?: Record<string, string>
    dsh?: { client?: { inject?: string[] } }
  }
  const inject = manifest.dsh?.client?.inject
  if (inject !== undefined) {
    manifest.dsh!.client!.inject = [...new Set(inject.map(name =>
      name === '@deepseek-ai/dsh-client-runtime'
        ? '@deepseek-ai/dsh-client-store'
        : name,
    ))]
  }
  for (const [name, range] of Object.entries(manifest.dependencies ?? {})) {
    if (!name.startsWith('@deepseek-ai/dsh-'))
      continue
    delete manifest.dependencies![name]
    manifest.peerDependencies ??= {}
    manifest.peerDependencies[name] = range
  }
  writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`)

  function adaptJavaScriptTree(dir: string): void {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const path = join(dir, entry.name)
      if (entry.isDirectory()) {
        if (entry.name !== 'node_modules')
          adaptJavaScriptTree(path)
        continue
      }
      if (!entry.isFile() || !entry.name.endsWith('.js'))
        continue
      const source = readFileSync(path, 'utf8')
      const adapted = source.replaceAll(
        '@deepseek-ai/dsh-client-runtime/client',
        '@deepseek-ai/dsh-client-store',
      )
      if (adapted !== source)
        writeFileSync(path, `${adapted.trimEnd()}\n`)
      if (adapted.includes('@deepseek-ai/dsh-client-runtime/client'))
        die(`${path}: split-client-v1 artifact still requests dsh-client-runtime/client`)
    }
  }
  adaptJavaScriptTree(dest)
}

/** Adapt direct RC slot registration to the alpha.1 injected-slot protocol. */
function adaptInjectedSlotsV1(dest: string): void {
  const manifest = JSON.parse(readFileSync(join(dest, 'package.json'), 'utf8')) as { name?: string }
  // alpha.1 的层级插槽要求贡献者通过 inject 等待父入口声明 children table。
  // dsh-tauri-session 0.5.3 仍直接 register(settings.section)，Loader 会在父入口
  // 尚未落账时拒绝整个插件图。适配固定发布制品，不修改官方核心或用户 profile。
  if (manifest.name === 'dsh-tauri-session') {
    const clientPath = join(dest, 'dist', 'client.js')
    const source = readFileSync(clientPath, 'utf8')
    const directRegistration = /([A-Za-z_$][\w$]*)\.effect\(\(\)=>\1\.slots\.register\((\{name:`settings\.section`,[\s\S]*?\},[A-Za-z_$][\w$]*)\),[A-Za-z_$][\w$]*\)/
    const adapted = source.replace(
      directRegistration,
      '$1.slots.inject("settings.section",()=>$1.slots.register($2))',
    )
    if (adapted === source)
      die(`${dest}: injected-slots-v1 dsh-tauri-session transform did not match`)
    writeFileSync(clientPath, `${adapted.trimEnd()}\n`)
  }
}

/** 构建单个 internal 插件：git 来源（克隆 → 装依赖 → 构建）或 npm 来源（拉产物）。 */
function buildPlugin(preset: InternalPlugin, root = BUNDLE_ROOT): void {
  for (const target of CORE_TARGETS)
    rmSync(join(root, target.pluginArtifactSet, preset.id), { recursive: true, force: true })

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

  for (const target of CORE_TARGETS) {
    const dest = collectBundle(preset, source, target.pluginArtifactSet, root)
    if (target.clientAbi === 'split-client-v1')
      adaptSplitClientV1(dest)
    if (target.slotProtocol === 'injected-slots-v1')
      adaptInjectedSlotsV1(dest)
    if (root === PRESET_BUNDLE_ROOT && preset.id === 'dsh-win-terminal-inspector') {
      if (target.providesWinTerminalInspector) {
        rmSync(dest, { recursive: true, force: true })
      }
      else {
        const manifestPath = join(dest, 'package.json')
        const manifest = JSON.parse(readFileSync(manifestPath, 'utf8')) as Record<string, unknown>
        manifest.dsh = { bundle: { patch: './cordis.patch.yml' } }
        writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`)
        writeFileSync(join(dest, 'cordis.patch.yml'), '- insert:\n    - id: win-terminal-inspector\n      name: dsh-win-terminal-inspector\n')
      }
    }
  }
  rmSync(temp, { recursive: true, force: true })
  console.log(`[prebuild] ${preset.id}: ${CORE_TARGETS.length} 个精确核心版本产物已就绪`)
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
  const internal = JSON.parse(readFileSync(INTERNAL_PLUGINS_FILE, 'utf8')) as InternalPlugin[]
  const presets = JSON.parse(readFileSync(PRESET_PLUGINS_FILE, 'utf8')) as InternalPlugin[]
  const requestedIds = new Set(
    (process.env.DSH_DESKTOP_BUNDLE_PLUGIN_IDS ?? '')
      .split(',')
      .map(id => id.trim())
      .filter(Boolean),
  )
  const selectedInternal = requestedIds.size === 0
    ? internal
    : internal.filter(plugin => requestedIds.has(plugin.id))
  const selectedPresets = requestedIds.size === 0
    ? presets
    : presets.filter(plugin => requestedIds.has(plugin.id))
  const unknownIds = [...requestedIds].filter(id =>
    !internal.some(plugin => plugin.id === id) && !presets.some(plugin => plugin.id === id),
  )
  if (unknownIds.length > 0)
    die(`指定了未知插件: ${unknownIds.join(', ')}`)
  if (internal.length === 0) {
    console.log('[prebuild] 内部插件清单为空，跳过')
    return
  }
  console.log(`[prebuild] 制备 ${selectedInternal.length} 个 internal 插件: ${selectedInternal.map(p => p.id).join(', ')}`)
  for (const plugin of selectedInternal) {
    buildPlugin(plugin)
  }
  console.log(`[prebuild] 制备 ${selectedPresets.length} 个首次引导插件的版本世代产物`)
  for (const plugin of selectedPresets)
    buildPlugin(plugin, PRESET_BUNDLE_ROOT)
  console.log(`[prebuild] 完成 → ${BUNDLE_ROOT}, ${PRESET_BUNDLE_ROOT}`)
}

main()
