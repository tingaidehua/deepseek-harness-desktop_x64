import { readdirSync, readFileSync, rmSync } from 'node:fs'
import { brotliDecompressSync } from 'node:zlib'
import { join } from 'pathe'

/** 删除 Tauri 中断构建留下的不可解压资源缓存，保留全部有效缓存。 */
export function healTauriAssetCache(targetDir: string): string[] {
  const removed: string[] = []
  for (const profile of ['debug', 'release']) {
    const buildDir = join(targetDir, profile, 'build')
    let buildEntries
    try {
      buildEntries = readdirSync(buildDir, { withFileTypes: true })
    }
    catch {
      continue
    }
    for (const buildEntry of buildEntries) {
      if (!buildEntry.isDirectory() || !buildEntry.name.startsWith('deepseek-harness-desktop-'))
        continue
      const cacheDir = join(buildDir, buildEntry.name, 'out', 'tauri-codegen-assets')
      let assets
      try {
        assets = readdirSync(cacheDir, { withFileTypes: true })
      }
      catch {
        continue
      }
      for (const asset of assets) {
        if (!asset.isFile())
          continue
        const path = join(cacheDir, asset.name)
        try {
          brotliDecompressSync(readFileSync(path))
        }
        catch {
          rmSync(path)
          removed.push(path)
        }
      }
    }
  }
  return removed
}
