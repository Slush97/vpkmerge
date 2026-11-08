// Run against an unmodified upstream SDK with Bun:
// bun probe-puppet-clock.ts /path/to/modlock/js/src
// Only the engine boundary is mocked; clock and placement use the real Puppet.
import assert from 'node:assert/strict'
import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { mock } from 'bun:test'

const source = resolve(process.argv[2] ?? '')
if (!process.argv[2]) throw new Error('Supply the upstream js/src directory')
mock.module(resolve(source, 'host.gen.ts'), () => ({
  addBot: () => undefined,
  holdModifierState: () => true,
  writeField: () => true,
  Player: class {},
}))
mock.module(resolve(source, 'players.ts'), () => ({ players: () => [] }))
const { Puppet, poseAt, trackTicks } = await import(pathToFileURL(resolve(source, 'puppet.ts')).href)

const track = {
  stepTicks: 1,
  x: [0, 0, 1284, 1284, 1284],
  y: [0, 0, 0, 0, 0], z: [0, 0, 0, 0, 0], yaw: [0, 0, 0, 0, 0],
}
const dt = 1 / 64
function exercise(prime: boolean) {
  const pawn = { entity: 1, position: { x: 0, y: 0, z: 0 } }
  const place = (position: typeof pawn.position) => {
    pawn.position = { ...position }
    return true
  }
  const player = { slot: 1, pawn: () => pawn, move: place, teleport: place }
  const puppet = new Puppet(player, track)
  const observations = []
  for (const start of [1, 2, 3]) {
    if (start > 1) {
      puppet.pause()
      puppet.seek(0)
      // Automatic SDK frames continue during setup, including the prior tick.
      puppet.advance(start - dt)
    }
    puppet.seek(0)
    if (prime) {
      puppet.pause()
      puppet.advance(start)
    }
    puppet.play()
    // SDK frame order: callbacks can start a pass, then puppets advance,
    // then the scenario's onFrame compares against (now - started) * 64.
    puppet.advance(start)
    puppet.advance(start + dt)
    const expectedTick = 1
    observations.push({ pass: observations.length + 1, expectedTick,
      puppetTick: puppet.tick,
      reportedDrift: Math.abs(pawn.position.x - poseAt(track, expectedTick).position.x) })
  }
  return observations
}

const original = exercise(false)
const primed = exercise(true)
assert.deepEqual(original.map(x => x.puppetTick), [1, 2, 2])
assert.deepEqual(original.map(x => x.reportedDrift), [0, 1284, 1284])
assert.deepEqual(primed.map(x => x.puppetTick), [1, 1, 1])
assert.deepEqual(primed.map(x => x.reportedDrift), [0, 0, 0])

function exerciseClockDomain(frameOffset: number, useFrameClock: boolean) {
  const pawn = { entity: 2, position: { x: 0, y: 0, z: 0 } }
  const place = (position: typeof pawn.position) => { pawn.position = { ...position }; return true }
  // Long enough to avoid clamping to the end of the track.
  const longTrack = { stepTicks: 1, x: Array(100).fill(0), y: Array(100).fill(0),
    z: Array(100).fill(0), yaw: Array(100).fill(0) }
  const puppet = new Puppet({ slot: 2, pawn: () => pawn, move: place, teleport: place }, longTrack)
  const ticks = []
  for (const start of [10, 20, 30]) {
    puppet.pause()
    puppet.seek(0)
    // The real fix starts inside onFrame, after the SDK's paused advance.
    if (useFrameClock) puppet.advance(start + frameOffset)
    puppet.advance(useFrameClock ? start + frameOffset : start)
    puppet.play()
    if (!useFrameClock) puppet.advance(start + frameOffset)
    puppet.advance(start + frameOffset + dt)
    ticks.push(puppet.tick)
  }
  return ticks
}
const wrongClockDomain = exerciseClockDomain(0.25, false)
const singleFrameClock = exerciseClockDomain(0.25, true)
assert.deepEqual(wrongClockDomain, [17, 17, 17])
assert.deepEqual(singleFrameClock, [1, 1, 1])
// A large offset must not affect playback when all work uses frame time.
assert.deepEqual(exerciseClockDomain(100, true), [1, 1, 1])
// A frame crossing the track end is valid: Puppet and poseAt clamp there.
const endPawn = { entity: 3, position: { x: 0, y: 0, z: 0 } }
const endPlace = (position: typeof endPawn.position) => { endPawn.position = position; return true }
const ending = new Puppet({ slot: 3, pawn: () => endPawn, move: endPlace, teleport: endPlace }, track)
ending.advance(1)
ending.play()
ending.advance(1.125)
assert.equal(ending.tick, Math.min(8, trackTicks(track)))
assert.equal(endPawn.position.x, poseAt(track, 8).position.x)
console.log(JSON.stringify({ source, original, primed, wrongClockDomain, singleFrameClock,
  scope: 'Actual SDK with mocked engine calls. Proves reset timing, not live-game smoothness.' }, null, 2))
