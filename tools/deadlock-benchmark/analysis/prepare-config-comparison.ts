// Build isolated benchmark inputs through Grimoire's actual preset generator
// and patcher. Steam userdata discovery is deliberately disabled for this tool.
import { mock } from 'bun:test'
import { readFileSync, writeFileSync, mkdirSync, existsSync } from 'node:fs'
import { resolve, join } from 'node:path'
import { createHash } from 'node:crypto'

const [rootArg, grimoireArg] = process.argv.slice(2)
if (!rootArg || !grimoireArg) throw new Error('Usage: prepare-config-comparison.ts experiment-dir grimoire-repo')
const root = resolve(rootArg), services = join(resolve(grimoireArg), 'electron/main/services')
if (existsSync(join(root, 'variants'))) throw new Error('Variant outputs already exist; use a fresh experiment')
mock.module(join(services, 'steamRoots.ts'), () => ({ getSteamRoots: () => [] }))
mock.module(join(services, 'deadlock.ts'), () => ({
  getGameinfoPath: (p: string) => join(p, 'game/citadel/gameinfo.gi'),
  getCitadelPath: (p: string) => join(p, 'game/citadel'),
}))
const { buildLatestRelease, latestAsPreset } = await import(join(services, 'performanceLatestCore.ts'))
const { applyPerformanceConfig, setExtraPresetResolver } = await import(join(services, 'performanceConfig.ts'))
const { getVideoPath } = await import(join(services, 'performanceVideo.ts'))
const inputs = JSON.parse(readFileSync(join(root, 'upstream-inputs.json'), 'utf8'))
const stock = readFileSync(join(root, 'game/game/citadel/gameinfo.gi'), 'utf8')
if (stock.includes('grimoire-perf')) throw new Error('Baseline contains an applied preset')
const video = readFileSync('/mnt/storage/SteamLibrary/steamapps/common/Deadlock/game/citadel/cfg/video.txt', 'utf8')
const hash = (text: string) => createHash('sha256').update(text).digest('hex')
// Same output and internal rendering resolution, no cap or VSync. Other
// author render settings remain treatment differences. No optional VPKs.
const controls: Record<string, string> = {
  defaultres: '2560', defaultresheight: '1440', fullscreen: '0',
  coop_fullscreen: '0', nowindowborder: '1', aspectratiomode: '0',
  mat_vsync: '0', fps_max: '0', mat_viewportscale: '1.000000',
  r_citadel_upscaling: '0', r_citadel_dlss_settings_mode: '0',
}
for (const id of ['sqooky-default', 'optilock-fps']) {
  const input = inputs[id]
  const built = buildLatestRelease({ presetId: id,
    refKind: id === 'optilock-fps' ? 'tag' : 'prose',
    ref: id === 'optilock-fps' ? 'v6.1d' : input.commit,
    commit: input.commit, date: id === 'optilock-fps' ? '2026-11-01' : '2026-11-04',
    baselineCommit: inputs.baseline.commit,
    baselineText: readFileSync(inputs.baseline.local, 'utf8'),
    configText: readFileSync(input.local, 'utf8'),
    videoText: id === 'optilock-fps' ? readFileSync(inputs['optilock-video'].local, 'utf8') : null,
    now: new Date(),
  })
  if (!built.ok) throw new Error(`${id}: ${built.error}`)
  const preset = latestAsPreset(built.release)
  const dir = join(root, 'variants', id), citadel = join(dir, 'game/citadel')
  mkdirSync(join(citadel, 'cfg'), { recursive: true })
  writeFileSync(join(citadel, 'gameinfo.gi'), stock)
  writeFileSync(join(citadel, 'cfg/video.txt'), video)
  if (getVideoPath(dir) !== join(citadel, 'cfg/video.txt')) throw new Error('Video path escaped isolation')
  setExtraPresetResolver((requested: string) => requested === id ? preset : null)
  const optIns = preset.optIn.filter((x: { group: string; key: string }) => x.group !== 'devtools' && x.key !== 'r_aspectratio').map((x: { key: string }) => x.key)
  const status = applyPerformanceConfig(dir, { presetId: id, version: 'latest', resetOverrides: true, optIns })
  if (status.state === 'error' || status.appliedPresetId !== id) throw new Error(JSON.stringify(status))
  let configured = readFileSync(join(citadel, 'cfg/video.txt'), 'utf8')
  for (const [key, value] of Object.entries(controls)) {
    const pattern = new RegExp(`("setting\\.${key}"\\s*")[^"]*(")`, 'g')
    if (pattern.test(configured)) configured = configured.replace(pattern, (_: string, a: string, b: string) => a + value + b)
    else configured = configured.replace(/}\s*$/, `\t"setting.${key}"\t"${value}"\n}\n`)
  }
  writeFileSync(join(citadel, 'cfg/video.txt'), configured)
  const info = readFileSync(join(citadel, 'gameinfo.gi'), 'utf8')
  writeFileSync(join(dir, 'preset.json'), JSON.stringify({
    id, version: preset.version, upstream: preset.upstream, release: built.release,
    controls, optional_vpks: false, gameinfo_sha256: hash(info), video_sha256: hash(configured),
    scope: 'Grimoire-filtered main config and supplied video settings, with shared resolution/upscaling/cap controls',
    patch_status: status,
  }, null, 2) + '\n')
  console.log(JSON.stringify({ id, version: preset.version, gameinfo_sha256: hash(info), video_sha256: hash(configured) }))
}
