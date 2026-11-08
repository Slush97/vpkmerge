import { addBot, after, Buttons, command, every, log, matchClock, onDamage, onDamaged, onFrame, onStart, onWorld, players, precache, show, trace, Layers, type Angles, type Player, type Timer, type Vector } from 'modlock'

// Placement is relative to the operator's hero. Record it for every comparison.
const HERO = 'hero_wraith'
const PERIOD = 4
const PASS_SECONDS = 60
const SETUP_SECONDS = 8
let facingA: Angles = { pitch: 0, yaw: 0, roll: 0 }
let facingB: Angles = { pitch: 0, yaw: 180, roll: 0 }
let actorSpots: Vector[] = []
let world = ''
let generation = 0
let running = false
let operator: Player | undefined
let actors: Player[] = []
let timers: Timer[] = []
let anchor: Vector | undefined
let castCount = 0
let hitCount = 0
let incomingCount = 0
let outgoingCount = 0
let bursts = 0
let demo = false
let demoAttempted = false
let demoTimer: Timer | undefined
let phase = 'Stand in a clear area, then type /bench_start'

function render(): void {
  if (!operator) return
  show(operator, <panel style={{ horizontalAlign: 'center', verticalAlign: 'top', margin: 30, padding: 12, background: '#101820e8' }}>
    <label style={{ fontSize: 26, color: '#ffffff', bold: true }}>{phase}</label>
  </panel>)
}

function cleanup(): void {
  for (const timer of timers) timer.cancel()
  timers = []
  for (const actor of actors) actor.removeBot()
  actors = []
}

function stop(reason: string): void {
  generation += 1
  running = false
  cleanup()
  phase = reason
  log(`FIGHT PROBE ${reason} game_time=${matchClock() ?? 'unknown'}`)
  render()
}

function spot(index: number): Vector { return actorSpots[index] }

function resetActors(): boolean {
  let ok = true
  for (let i = 0; i < actors.length; i += 1) {
    const actor = actors[i]
    const pawn = actor.pawn()
    if (!pawn) { ok = false; continue }
    if (pawn.health <= 0 && !actor.respawn()) ok = false
    if (!actor.teleport(spot(i), i < 2 ? facingA : facingB)) ok = false
    const ability = actor.abilities().find((value) => value.slot === 0)
    log(`FIGHT PROBE actor=${i + 1} ability1=${ability?.name ?? 'unknown'} pawn=${pawn.entity} team=${pawn.team} health=${pawn.health}`)
    if (!ability || !actor.setAbility({ ability: ability.name, upgrades: 1 })) ok = false
  }
  return ok
}

function burst(): void {
  bursts += 1
  if (bursts > 1 && bursts % 3 === 1 && !resetActors()) { stop('INVALID: bot reset failed'); return }
  let accepted = 0
  for (let i = 0; i < actors.length; i += 1) {
    const pawn = actors[i].pawn()
    if (!pawn || pawn.health <= 0) { stop('INVALID: actor dead or missing'); return }
    const ability = actors[i].abilities().find((value) => value.slot === 0)
    if (ability && !actors[i].refreshAbility(ability.name)) { stop('INVALID: ability refresh failed'); return }
    if (actors[i].drive({ facing: i < 2 ? facingA : facingB, forward: 0, left: 0,
      held: bursts % 3 === 0 ? Buttons.reload : Buttons.attack, pressed: Buttons.ability1 })) accepted += 1
  }
  log(`FIGHT PROBE burst accepted=${accepted}/${actors.length} game_time=${matchClock() ?? 'unknown'}`)
  if (accepted !== actors.length) stop('INVALID: bot input rejected')
}

onFrame((frame) => {
  if (!running) return
  for (const sample of frame.movement) {
    if (actors.some((actor) => actor.slot === sample.player.slot)) castCount += sample.casts.length
  }
})
onDamage((hit) => {
  if (!running) return
  const incoming = actors.some((actor) => actor.pawn()?.entity === hit.victim)
  if (actors.some((actor) => actor.pawn()?.entity === hit.attacker)) outgoingCount += 1
  if (incoming) { incomingCount += 1; return { amount: Math.min(hit.amount, 1) } }
})
onDamaged((hit) => {
  if (running && actors.some((actor) => actor.pawn()?.entity === hit.attacker)) hitCount += 1
})

onStart((args) => { demo = args.includes('demo'); log(`FIGHT PROBE precache accepted=${precache({ heroes: [HERO] })}`) })
onWorld((map) => {
  if (running) stop('INVALID: world changed')
  else { generation += 1; cleanup() }
  world = map
  operator = undefined
  anchor = undefined
  demoTimer?.cancel()
  if (demo && !demoAttempted && /(^|\/)dl_midtown(\.vpk)?$/i.test(world)) {
    let readyTicks = 0
    demoTimer = every(1, () => {
      const human = players().find((connection) => !connection.bot && connection.ready && connection.player.pawn()?.cameraPosition)
      readyTicks = human ? readyTicks + 1 : 0
      if (human && readyTicks >= 5) {
        demoAttempted = true
        demoTimer?.cancel()
        startFight(human.player)
      }
    })
  }
})

command('bench_stop', (player) => {
  if (operator && player.slot !== operator.slot) return
  stop('STOPPED')
  player.unwatch()
})

command('bench_status', (player) => { player.chat(`Fight probe: ${phase}`) })

function startFight(player: Player): void {
  if (running) { player.chat('Fight probe is already running.') ; return }
  operator = player
  if (!/(^|\/)dl_midtown(\.vpk)?$/i.test(world)) { phase = `Wrong map: ${world}; load dl_midtown.`; log(`FIGHT PROBE INVALID ${phase}`); render(); return }
  const pawn = player.pawn()
  if (!pawn) { phase = 'No hero. Spawn in a clear area before starting.'; log(`FIGHT PROBE INVALID ${phase}`); render(); return }
  const camera = pawn.cameraPosition
  if (!camera) { phase = 'Camera position unavailable; move slightly and retry.'; log(`FIGHT PROBE INVALID ${phase}`); render(); return }
  const yaw = pawn.cameraAngles.yaw
  const radians = yaw * Math.PI / 180
  const fx = Math.cos(radians), fy = Math.sin(radians)
  anchor = { x: pawn.position.x + fx * 500, y: pawn.position.y + fy * 500, z: pawn.position.z }
  facingA = { pitch: 0, yaw, roll: 0 }
  facingB = { pitch: 0, yaw: yaw + 180, roll: 0 }
  actorSpots = []
  for (let i = 0; i < 4; i += 1) {
    const along = i < 2 ? -180 : 180
    const side = i % 2 ? 90 : -90
    const x = anchor.x + fx * along - fy * side
    const y = anchor.y + fy * along + fx * side
    const ground = trace({ start: { x, y, z: anchor.z + 128 }, end: { x, y, z: anchor.z - 256 }, layers: Layers.solid, ignore: [pawn.entity] })
    if (ground && (ground.startSolid || ground.normal.z < 0.6)) { phase = 'No clear floor for the bots. Reposition and retry.'; log(`FIGHT PROBE INVALID ${phase}`); render(); return }
    if (!ground) log(`FIGHT PROBE floor unavailable at actor=${i + 1}; using operator height`)
    actorSpots.push({ x, y, z: ground ? ground.position.z + 2 : pawn.position.z + 2 })
  }
  if (!player.freeCamera(camera, pawn.cameraAngles)) { stop('INVALID: camera request rejected'); return }
  running = true
  castCount = hitCount = incomingCount = outgoingCount = bursts = 0
  const current = ++generation
  phase = 'SETUP: four scripted Wraith bots; do not move camera'
  render()
  log(`FIGHT PROBE anchor=${anchor.x},${anchor.y},${anchor.z} camera=${camera.x},${camera.y},${camera.z} pitch=${pawn.cameraAngles.pitch} yaw=${yaw} spots=${JSON.stringify(actorSpots)} map=${world}`)
  for (let i = 0; i < 4; i += 1) {
    const bot = addBot({ name: `Fight probe ${i + 1}`, team: i < 2 ? 2 : 3, hero: HERO, position: spot(i) })
    if (!bot) { stop('INVALID: could not add all bots'); return }
    actors.push(bot)
    log(`FIGHT PROBE bot=${i + 1} slot=${bot.slot} watchMovement=${bot.watchMovement(true)}`)
  }
  timers.push(every(1, () => {
    if (!players().some(({ player: connected }) => connected.slot === player.slot)) stop('INVALID: operator disconnected')
  }))
  timers.push(after(SETUP_SECONDS, () => {
    if (current !== generation) return
    if (!resetActors()) { stop('INVALID: bot setup failed'); return }
    phase = 'WARM-UP: start capture now'
    render()
    log(`FIGHT PROBE WARMUP game_time=${matchClock() ?? 'unknown'}`)
    burst()
    if (!running) return
    timers.push(every(PERIOD, () => { if (current === generation) burst() }))
    timers.push(after(PASS_SECONDS, () => {
      if (current !== generation) return
      if (!resetActors()) { stop('INVALID: bot reset failed'); return }
      log(`FIGHT PROBE WARMUP observed_casts=${castCount} observed_hits=${hitCount} incoming_damage_events=${incomingCount} outgoing_damage_events=${outgoingCount}`)
      castCount = hitCount = incomingCount = outgoingCount = 0
      bursts = 0
      phase = 'BENCH START'
      render()
      log(`FIGHT PROBE START game_time=${matchClock() ?? 'unknown'}`)
      timers.push(after(PASS_SECONDS, () => {
        if (current !== generation) return
        phase = castCount > 0 && hitCount > 0
          ? 'BENCH END: stop capture; /bench_stop restores camera'
          : 'INVALID: no verified casts or hits; /bench_stop restores camera'
        render()
        log(`FIGHT PROBE END game_time=${matchClock() ?? 'unknown'} observed_casts=${castCount} observed_hits=${hitCount} incoming_damage_events=${incomingCount} outgoing_damage_events=${outgoingCount} valid_combat=${castCount > 0 && hitCount > 0}`)
        if (demo) {
          phase = 'DEMO continuing; not capturing; /bench_stop to end'
          render()
          log('FIGHT PROBE DEMO continuing; not capturing; /bench_stop to end')
        } else {
          running = false
          cleanup()
        }
      }))
    }))
  }))
}

command('bench_start', startFight)
