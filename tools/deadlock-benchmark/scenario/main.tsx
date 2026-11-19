import { after, command, every, log, matchClock, onWorld, players, show, type Player, type Timer } from 'modlock'

const CAMERA = { x: 0, y: -380, z: 170 }
const FACING = { pitch: 18, yaw: 90, roll: 0 }
let world = ''
let generation = 0
let phase = 'Type /bench_start after loading the lab map'
let operator: Player | undefined
let running = false
let disconnectCheck: Timer | undefined

function render(): void {
  if (!operator) return
  show(operator, <panel style={{ horizontalAlign: 'center', verticalAlign: 'top', margin: 30, padding: 12, background: '#101820e8' }}>
    <label style={{ fontSize: 28, color: '#ffffff', bold: true }}>{phase}</label>
  </panel>)
}

onWorld((map) => {
  generation += 1
  disconnectCheck?.cancel()
  if (running) log('BENCH INVALID: world changed during run')
  world = map
  running = false
  operator = undefined
  phase = 'Type /bench_start after loading the lab map'
})

command('bench_start', (player) => {
  if (running) return
  operator = player
  if (!/(^|\/)dl_benchmark_lab_(0|16)(\.vpk)?$/i.test(world)) {
    phase = `Wrong map: ${world}. Load dl_benchmark_lab_0 or dl_benchmark_lab_16.`
    render()
    return
  }
  if (!player.freeCamera(CAMERA, FACING)) {
    phase = 'Camera request failed. Run invalid.'
    log('BENCH INVALID: freeCamera returned false')
    render()
    return
  }
  running = true
  const current = ++generation
  phase = 'WARM-UP: start capture now; do not move the camera'
  render()
  log(`BENCH WARMUP map=${world} game_time=${matchClock() ?? 'unknown'}`)
  disconnectCheck = every(1, () => {
    if (!players().some(({ player: connected }) => connected.slot === player.slot)) {
      generation += 1
      running = false
      disconnectCheck?.cancel()
      operator = undefined
      log('BENCH INVALID: operator disconnected')
    }
  })
  after(30, () => {
    if (current !== generation) return
    phase = 'BENCH START'
    render()
    log(`BENCH START game_time=${matchClock() ?? 'unknown'}`)
    after(60, () => {
      if (current !== generation) return
      phase = 'BENCH END: stop capture after this marker'
      render()
      log(`BENCH END game_time=${matchClock() ?? 'unknown'}`)
      running = false
      disconnectCheck?.cancel()
    })
  })
})
