import { Buffer } from 'node:buffer'
import { mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { brotliCompressSync } from 'node:zlib'
import { afterEach, describe, expect, it } from 'vitest'
import { healTauriAssetCache } from '../scripts/tauri-asset-cache'

const roots: string[] = []

afterEach(() => {
  for (const root of roots.splice(0)) {
    try {
      rmSync(root, { recursive: true, force: true })
    }
    catch {
      // The test owns only disposable temporary directories; the OS may still hold a handle.
    }
  }
})

describe('healTauriAssetCache', () => {
  it('removes corrupt interrupted writes and preserves valid Brotli assets', () => {
    const root = join(tmpdir(), `dsh-tauri-cache-${process.pid}-${Date.now()}`)
    roots.push(root)
    const cache = join(root, 'release', 'build', 'deepseek-harness-desktop-test', 'out', 'tauri-codegen-assets')
    mkdirSync(cache, { recursive: true })
    const valid = join(cache, 'valid.js')
    const corrupt = join(cache, 'corrupt.js')
    writeFileSync(valid, brotliCompressSync(Buffer.from('export const ready = true')))
    writeFileSync(corrupt, Buffer.from('interrupted'))

    expect(healTauriAssetCache(root)).toHaveLength(1)
    expect(healTauriAssetCache(root)).toEqual([])
    expect(readFileSync(valid).length).toBeGreaterThan(0)
    expect(() => readFileSync(corrupt)).toThrow()
  })
})
