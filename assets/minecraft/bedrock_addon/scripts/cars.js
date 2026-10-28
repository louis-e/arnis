// Cars on the lanes Arnis planned, driven as the Java pack's cars functions drive them:
// speeds are thousandths of a block per step and a step is every second tick. The rules
// Arnis writes into the Java pack come in the data; the rest are the pack's own numbers.
import { world } from "@minecraft/server";
import { Bits, ahead, distance, number, numbers, randInt, turnBetween, warn, yawOf } from "./util.js";

const FAMILY = "arnis_car";
const BRAKE = "arnis:brake";
/** Cars this close to a player move. */
const DRIVE_REACH = 96;
/** A car placed at the start appears once a player comes this close. */
const PLACE_REACH = 104;
/** Standing this many steps with nothing to wait for is stuck. */
const STUCK = 600;
/** A respawned car starts no closer to a player than this, and this far from other cars. */
const RESPAWN_CLEAR = 40;
const RESPAWN_GAP = 12;
const CELL = 16;

function cellKey(x, z) {
  return Math.floor(x / CELL) * 4194304 + Math.floor(z / CELL);
}

/** A small number that tells cars apart when they give way to each other. */
function tieBreak(id) {
  let h = 0;
  for (let i = 0; i < id.length; i++) h = (h * 31 + id.charCodeAt(i)) | 0;
  return Math.abs(h) % 32;
}

export class Cars {
  constructor(dimension, data, glide) {
    this.dim = dimension;
    this.glide = glide;
    this.k = data.size;
    this.accel = data.accel;
    this.raw = data.lanes;
    this.lanes = new Array(data.lanes.length);
    this.start = data.start;
    this.bodies = data.bodies;
    this.colours = data.colours;
    this.rules = data.rules;
    // Cars this far out count as traffic: as far as a car that moves looks ahead, and at
    // least as far as a car is placed or brought back with room around it.
    const sight = Math.max(...this.rules.look.map(([at, reach]) => at + reach)) * this.k;
    this.seeReach = Math.max(DRIVE_REACH + sight, PLACE_REACH + 3 * this.k, DRIVE_REACH + RESPAWN_GAP);
    this.cars = new Map();
    this.grid = new Map();
    this.placed = new Bits("arnis:cars_placed", this.start.length);
    this.pending = number(world.getDynamicProperty("arnis:cars_pending"));
    this.steps = 0;
  }

  /** A lane, read from its lists the first time a car needs it. */
  lane(s) {
    let lane = this.lanes[s];
    if (lane) return lane;
    const raw = this.raw[s];
    const p = numbers(raw.p);
    const n = p.length / 3;
    const x = new Float64Array(n);
    const y = new Float64Array(n);
    const z = new Float64Array(n);
    let ax = 0;
    let ay = 0;
    let az = 0;
    for (let i = 0; i < n; i++) {
      ax += p[3 * i];
      ay += p[3 * i + 1];
      az += p[3 * i + 2];
      x[i] = ax / 10;
      y[i] = ay / 10;
      z[i] = az / 10;
    }
    const caps = new Int32Array(n).fill(-1);
    const m = numbers(raw.m);
    for (let k = 0; k + 1 < m.length; k += 2) caps[m[k]] = m[k + 1];
    const turns = numbers(raw.n);
    const next = [];
    for (let k = 0; k + 2 < turns.length; k += 3) next.push([turns[k], turns[k + 1], turns[k + 2]]);
    lane = { n, x, y, z, caps, next, speed: raw.v };
    this.lanes[s] = lane;
    // Read once, the lists are only memory.
    delete raw.p;
    delete raw.m;
    delete raw.n;
    return lane;
  }

  loaded(at) {
    return this.dim.isChunkLoaded(at);
  }

  step(players) {
    this.steps++;
    this.gather(players);
    for (const car of [...this.cars.values()]) {
      if (!car.drives || !this.cars.has(car.id)) continue;
      try {
        this.drive(car, players);
      } catch (e) {
        warn("driving a car", e);
        if (!car.e.isValid) this.forget(car);
      }
    }
    if (this.steps % 10 === 0) {
      this.place(players);
      this.respawn(players);
    }
  }

  /** Every car near a player, each picked up again from what it carries if it is new here. */
  gather(players) {
    const found = new Map();
    for (const p of players) {
      for (const e of this.dim.getEntities({
        families: [FAMILY],
        location: p.location,
        maxDistance: this.seeReach,
      })) {
        found.set(e.id, e);
      }
    }
    const cars = new Map();
    this.grid.clear();
    for (const [id, e] of found) {
      let car = this.cars.get(id);
      if (car) {
        car.e = e;
      } else {
        try {
          car = this.adopt(e);
        } catch (err) {
          warn("picking up a car", err);
        }
      }
      if (!car) continue;
      car.drives = players.some((p) => distance(p.location, car) <= DRIVE_REACH);
      cars.set(id, car);
      this.index(car);
    }
    this.cars = cars;
  }

  adopt(e) {
    const s = e.getDynamicProperty("arnis:s");
    const i = e.getDynamicProperty("arnis:i");
    const p = e.getDynamicProperty("arnis:p");
    const valid =
      Number.isInteger(s) && s >= 0 && s < this.raw.length && Number.isInteger(i) && typeof p === "number";
    if (!valid || i < 0 || i >= this.lane(s).n) {
      e.remove();
      this.lost();
      return undefined;
    }
    const at = e.location;
    const car = {
      e,
      id: e.id,
      s,
      i,
      p,
      j: e.getDynamicProperty("arnis:j") === true,
      x: at.x,
      y: at.y,
      z: at.z,
      yaw: e.getRotation().y,
      c: 0,
      w: 0,
      q: 0,
      b: e.getProperty(BRAKE) === true ? 1 : 0,
      n: tieBreak(e.id),
    };
    this.enter(car);
    return car;
  }

  index(car) {
    const key = cellKey(car.x, car.z);
    car.cell = key;
    let list = this.grid.get(key);
    if (!list) this.grid.set(key, (list = []));
    list.push(car);
  }

  unindex(car) {
    const list = this.grid.get(car.cell);
    if (!list) return;
    const at = list.indexOf(car);
    if (at >= 0) list.splice(at, 1);
  }

  /** Whether a car other than `self` is within `reach` of `at`, and passes `test`. */
  any(at, reach, self, test) {
    const cx = Math.floor(at.x / CELL);
    const cz = Math.floor(at.z / CELL);
    const span = Math.ceil(reach / CELL);
    for (let dx = -span; dx <= span; dx++) {
      for (let dz = -span; dz <= span; dz++) {
        const list = this.grid.get((cx + dx) * 4194304 + cz + dz);
        if (!list) continue;
        for (const o of list) {
          if (o !== self && distance(o, at) <= reach && (!test || test(o))) return true;
        }
      }
    }
    return false;
  }

  /** How close the car ahead is: 3 stop a couple of metres behind it, 2 and 1 ease off. */
  look(car) {
    const same = (o) => turnBetween(o.yaw, car.yaw) <= this.rules.same_way;
    for (const [at, reach, close] of this.rules.look) {
      if (this.any(ahead(car, car.yaw, at * this.k), reach * this.k, car, same)) return close;
    }
    return 0;
  }

  drive(car, players) {
    const k = this.k;
    let t = car.v;
    const half = Math.floor(car.v / 2);
    const quarter = Math.floor(car.v / 4);
    if (car.m >= 0) t = Math.min(t, car.m);
    const close = this.look(car);
    if (close === 2) t = Math.min(t, quarter);
    if (close === 1) t = Math.min(t, half);
    // Crossing a junction, a car close ahead is a conflict to give way to for a while; on a
    // lane it is a queue to wait in.
    let jam = false;
    if (close === 3) {
      if (car.j) jam = true;
      else t = 0;
    }
    // Crossing traffic right in front: give way, but not for ever, since cars can block
    // each other in a circle there. Which goes first is down to its number.
    let give = jam;
    const [crossAt, crossReach] = this.rules.crossing;
    if (t >= 1 && this.any(ahead(car, car.yaw, crossAt * k), crossReach * k, car)) give = true;
    car.w = give ? car.w + 1 : 0;
    if (give && car.w <= car.n + this.rules.give_way) t = 0;
    // Never through a player.
    const [nearAt, farAt, reach] = this.rules.player;
    const near = ahead(car, car.yaw, nearAt * k);
    const far = ahead(car, car.yaw, farAt * k);
    const blocks = (p) => distance(p.location, near) <= reach * k || distance(p.location, far) <= reach * k;
    if (players.some((p) => p.walks && blocks(p))) {
      t = 0;
    }
    // Gentle on the throttle, firm on the brake.
    const was = car.c;
    car.c = Math.min(car.c + this.accel, t);
    // Standing for a minute is stuck: it leaves and another car comes.
    car.q = car.c >= 1 ? 0 : car.q + 1;
    if (car.q >= STUCK) {
      this.despawn(car);
      return;
    }
    // Bright while slowing down or standing, dim while driving on.
    const b = car.c < was || car.c <= 0 ? 1 : 0;
    if (b !== car.b) {
      car.b = b;
      car.e.setProperty(BRAKE, b === 1);
    }
    if (car.c <= 0) return;
    this.move(car, car.c * 0.001);
  }

  move(car, d) {
    const target = { x: car.tx, y: car.ty, z: car.tz };
    // A target in a chunk that is not loaded waits, rather than the car leaving the world it can be seen in.
    if (!this.loaded(target)) return;
    const from = { x: car.x, y: car.y, z: car.z };
    const dx = target.x - from.x;
    const dy = target.y - from.y;
    const dz = target.z - from.z;
    const dist = Math.hypot(dx, dy, dz);
    if (dist <= d) {
      car.x = target.x;
      car.y = target.y;
      car.z = target.z;
      if (!this.arrive(car)) return;
    } else {
      car.x += (dx / dist) * d;
      car.y += (dy / dist) * d;
      car.z += (dz / dist) * d;
      if (Math.hypot(dx, dz) > 1e-6) car.yaw = yawOf(dx, dz);
    }
    this.glide.move(car.e, from, car, car.yaw);
    const key = cellKey(car.x, car.z);
    if (key !== car.cell) {
      this.unindex(car);
      this.index(car);
    }
  }

  /** At the target: on to the next point, or onto the next lane. False if the car left. */
  arrive(car) {
    car.j = false;
    car.i += 1;
    if (car.i >= this.lane(car.s).n) return this.turn(car);
    this.target(car);
    this.save(car);
    return true;
  }

  turn(car) {
    const next = this.lane(car.s).next;
    if (next.length === 0) {
      this.despawn(car);
      return false;
    }
    const [s, i, crossing] = next[randInt(0, 999) % next.length];
    car.s = s;
    car.i = i;
    car.j = crossing === 1;
    this.enter(car);
    this.save(car);
    return true;
  }

  /** The new lane's speed, at this driver's pace. */
  enter(car) {
    car.v = Math.floor((this.lane(car.s).speed * car.p) / 1000);
    this.target(car);
  }

  target(car) {
    const lane = this.lane(car.s);
    car.tx = lane.x[car.i];
    car.ty = lane.y[car.i];
    car.tz = lane.z[car.i];
    car.m = lane.caps[car.i];
  }

  save(car) {
    car.e.setDynamicProperty("arnis:s", car.s);
    car.e.setDynamicProperty("arnis:i", car.i);
    car.e.setDynamicProperty("arnis:j", car.j);
  }

  forget(car) {
    this.unindex(car);
    this.cars.delete(car.id);
  }

  despawn(car) {
    this.forget(car);
    try {
      if (car.e.isValid) car.e.remove();
    } catch (e) {
      warn("removing a car", e);
    }
    this.lost();
  }

  lost() {
    this.pending += 1;
    this.savePending();
  }

  savePending() {
    world.setDynamicProperty("arnis:cars_pending", this.pending);
  }

  spawn(name, at, s, i, pace) {
    const lane = this.lane(s);
    const yaw = yawOf(lane.x[i] - at.x, lane.z[i] - at.z);
    const e = this.dim.spawnEntity(`arnis:${name}`, at, { initialRotation: yaw });
    e.setDynamicProperty("arnis:p", pace);
    const car = {
      e,
      id: e.id,
      s,
      i,
      p: pace,
      j: false,
      x: at.x,
      y: at.y,
      z: at.z,
      yaw,
      c: 0,
      w: 0,
      q: 0,
      b: 0,
      n: tieBreak(e.id),
      drives: false,
    };
    this.enter(car);
    this.save(car);
    this.cars.set(car.id, car);
    this.index(car);
    return car;
  }

  /** The cars Arnis placed, each brought in when a player first comes near its spot. */
  place(players) {
    let changed = false;
    for (let idx = 0; idx < this.start.length; idx++) {
      if (this.placed.has(idx)) continue;
      const [s, i, x, y, z, body, pace] = this.start[idx];
      const at = { x, y, z };
      if (!players.some((p) => distance(p.location, at) <= PLACE_REACH) || !this.loaded(at)) continue;
      this.placed.add(idx);
      changed = true;
      // Another car got there first; this one comes back elsewhere instead.
      if (this.any(at, 3 * this.k)) {
        this.pending += 1;
        continue;
      }
      try {
        const car = this.spawn(this.bodies[body], at, s, i, pace);
        // Already on its way, as the Java pack places it.
        car.c = car.m >= 0 ? Math.min(car.v, car.m) : car.v;
      } catch (e) {
        warn("placing a car", e);
        this.pending += 1;
      }
    }
    if (changed) {
      this.placed.save();
      this.savePending();
    }
  }

  /** A kind of car, each as often as its weight. */
  pickKind() {
    const kinds = this.rules.kinds;
    let roll = randInt(0, kinds.reduce((sum, [, weight]) => sum + weight, 0) - 1);
    for (const [kind, weight] of kinds) {
      if (roll < weight) return kind;
      roll -= weight;
    }
    return kinds[0][0];
  }

  /** Once a second, a few tries at bringing back a car that left. */
  respawn(players) {
    for (let tries = 0; tries < 3 && this.pending > 0 && this.raw.length > 0; tries++) {
      const s = randInt(0, 999999) % this.raw.length;
      // Not on a lane no car starts on, as a stub or a scrap of road with nowhere to go.
      if (this.raw[s].o === 1) continue;
      const lane = this.lane(s);
      const at = { x: lane.x[0], y: lane.y[0], z: lane.z[0] };
      // At the start of a lane, out of sight of anyone close but near enough to someone to be worth it.
      if (!this.loaded(at)) continue;
      if (players.some((p) => distance(p.location, at) <= RESPAWN_CLEAR)) continue;
      if (!players.some((p) => distance(p.location, at) <= DRIVE_REACH)) continue;
      if (this.any(at, RESPAWN_GAP)) continue;
      const kind = this.pickKind();
      const colours = this.colours[kind];
      const colour = colours[randInt(0, 9999) % colours.length];
      try {
        this.spawn(`car_${kind}_${colour}`, at, s, 1, randInt(...this.rules.pace));
      } catch (e) {
        warn("bringing back a car", e);
        continue;
      }
      this.pending -= 1;
      this.savePending();
    }
  }
}
