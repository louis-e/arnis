// Trains on the routes Arnis planned, run as the Java pack's trains functions run them: each
// train is where its timetable says it is now, so it keeps no state of its own, and its
// carriages are set down behind the front. Distances are thousandths of a block.
import { world } from "@minecraft/server";
import { distance, mod, number, numbers, warn } from "./util.js";

const FAMILY = "arnis_train";
const RAD = Math.PI / 180;
/** A route further than this from every player is out of the loaded chunks anyway. */
const NEAR = 512;
/** Characters of routes and timetables read in one step, so a big network loads over a few. */
const READ_BUDGET = 60000;

function decodeRoute(raw) {
  const p = numbers(raw.p);
  const n = p.length / 4;
  const x = new Float64Array(n);
  const y = new Float64Array(n);
  const z = new Float64Array(n);
  const h = new Float64Array(n);
  let ax = 0;
  let ay = 0;
  let az = 0;
  let ah = 0;
  for (let i = 0; i < n; i++) {
    ax += p[4 * i];
    ay += p[4 * i + 1];
    az += p[4 * i + 2];
    ah += p[4 * i + 3];
    x[i] = ax / 100;
    y[i] = ay / 1000;
    z[i] = az / 100;
    h[i] = ah / 10;
  }
  return { n, x, y, z, h, loop: raw.loop };
}

/** A timetable from its first value and (step, repeats) pairs. */
function decodeSteps(text, count) {
  const runs = numbers(text);
  const out = new Int32Array(count);
  let k = 0;
  let v = runs[0];
  out[k++] = v;
  for (let r = 1; r + 1 < runs.length && k < count; r += 2) {
    for (let j = 0; j < runs[r + 1] && k < count; j++) {
      v += runs[r];
      out[k++] = v;
    }
  }
  return out;
}

export class Trains {
  constructor(dimension, data, glide) {
    this.dim = dimension;
    this.glide = glide;
    this.raw = data.routes;
    this.routes = new Array(data.routes.length);
    this.services = data.services;
    this.timetables = new Array(data.services.length);
    this.bodies = data.bodies;
    this.now = number(world.getDynamicProperty("arnis:trains_now"));
    this.carriages = new Map();
    this.seen = new Map();
    this.budget = 0;
  }

  /** Whether service `t` has its route and timetable read, reading them if the step has room. */
  ready(t) {
    const service = this.services[t];
    const r = service.route;
    if (this.routes[r] && this.timetables[t]) return true;
    if (this.budget <= 0) return false;
    // Read once, the lists are only memory.
    if (!this.routes[r]) {
      this.budget -= this.raw[r].p.length;
      this.routes[r] = decodeRoute(this.raw[r]);
      delete this.raw[r].p;
    }
    if (!this.timetables[t]) {
      this.budget -= service.steps.length;
      this.timetables[t] = decodeSteps(service.steps, service.count);
      delete service.steps;
    }
    return true;
  }

  point(route, i) {
    return { x: route.x[i], y: route.y[i], z: route.z[i] };
  }

  step(players) {
    // One clock for every timetable, so a train is where it would be had it run all along.
    this.now++;
    this.budget = READ_BUDGET;
    for (let t = 0; t < this.services.length; t++) {
      const service = this.services[t];
      const [x0, z0, x1, z1] = this.raw[service.route].box;
      const near = players.some(
        ({ location: p }) => p.x > x0 - NEAR && p.x < x1 + NEAR && p.z > z0 - NEAR && p.z < z1 + NEAR,
      );
      if (!near || !this.ready(t)) continue;
      for (const train of service.trains) {
        try {
          this.run(t, service, train);
        } catch (e) {
          warn("running a train", e);
        }
      }
    }
    if (this.now % 10 === 0) {
      // With no one about nothing was put anywhere, so nothing counts as left behind.
      if (players.length > 0) this.sweep();
      world.setDynamicProperty("arnis:trains_now", this.now);
    }
  }

  run(t, service, train) {
    const route = this.routes[service.route];
    const steps = this.timetables[t];
    const front = steps[mod(this.now + train.phase, service.count)];
    const { visible, period } = service;
    // Nothing to draw while the middle of the train is out of loaded chunks.
    let middle = front - service.middle;
    if (period > 0) middle = mod(middle, period);
    middle = Math.min(Math.max(middle, 0), visible);
    if (!this.dim.isChunkLoaded(this.point(route, Math.floor(middle / 1000)))) return;
    for (const [c, body, offset] of train.cars) {
      let at = front - offset;
      if (period > 0) at = mod(at, period);
      // Off either end it is out of the area: taken away as it leaves.
      if (at < 0) {
        if (at > -8001) this.gone(c, this.point(route, 0));
        continue;
      }
      if (at > visible) {
        if (at - visible <= 8000) this.gone(c, this.point(route, route.n - 1));
        continue;
      }
      const i = Math.floor(at / 1000);
      this.put(c, body, route, i, (at % 1000) / 1000);
    }
  }

  /** Carriage `c` between point i and the next, a fraction f of the way, facing along the track. */
  put(c, body, route, i, f) {
    const p = this.point(route, i);
    if (!this.dim.isChunkLoaded(p)) return;
    const yaw = route.h[i];
    const next = i + 1 < route.n ? i + 1 : route.loop ? 0 : i;
    const at = {
      x: p.x - Math.sin(yaw * RAD) * f,
      y: p.y + (route.y[next] - p.y) * f,
      z: p.z + Math.cos(yaw * RAD) * f,
    };
    if (!this.dim.isChunkLoaded(at)) return;
    let e = this.carriages.get(c);
    if (!e?.isValid) e = this.find(c, at);
    if (e) {
      this.glide.move(e, e.location, at, yaw);
    } else {
      // Not there yet: it has just come into view, or into the area.
      e = this.dim.spawnEntity(`arnis:${this.bodies[body]}`, at, { initialRotation: yaw });
      e.setDynamicProperty("arnis:c", c);
    }
    this.carriages.set(c, e);
    this.seen.set(c, this.now);
  }

  find(c, at) {
    for (const e of this.dim.getEntities({ families: [FAMILY], location: at, maxDistance: 12 })) {
      if (e.getDynamicProperty("arnis:c") === c) return e;
    }
    return undefined;
  }

  gone(c, end) {
    const e = this.carriages.get(c);
    if (!e) return;
    if (e.isValid && distance(e.location, end) <= 24) e.remove();
    if (!e.isValid) {
      this.carriages.delete(c);
      this.seen.delete(c);
    }
  }

  /**
   * Carriages no timetable put anywhere lately: left behind when their chunk unloaded, or
   * of a train that has since gone out of the loaded chunks.
   */
  sweep() {
    const fresh = this.now - 10;
    for (const e of this.dim.getEntities({ families: [FAMILY] })) {
      const c = e.getDynamicProperty("arnis:c");
      const mine = this.carriages.get(c);
      if (mine?.id === e.id && (this.seen.get(c) ?? -Infinity) >= fresh) continue;
      if (mine?.id === e.id) {
        this.carriages.delete(c);
        this.seen.delete(c);
      }
      e.remove();
    }
  }
}
