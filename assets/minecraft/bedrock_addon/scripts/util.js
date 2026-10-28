import { system, world } from "@minecraft/server";

const RAD = Math.PI / 180;

/** The yaw that faces along (dx, dz): 0 is south, 90 west. */
export function yawOf(dx, dz) {
  return Math.atan2(-dx, dz) / RAD;
}

/** The point `d` blocks ahead of `at` along `yaw`, level. */
export function ahead(at, yaw, d) {
  return { x: at.x - Math.sin(yaw * RAD) * d, y: at.y, z: at.z + Math.cos(yaw * RAD) * d };
}

/** How far apart two yaws are, 0 to 180 degrees. */
export function turnBetween(a, b) {
  const d = Math.abs(a - b) % 360;
  return d > 180 ? 360 - d : d;
}

export function distance(a, b) {
  return Math.hypot(a.x - b.x, a.y - b.y, a.z - b.z);
}

/** A whole number from lo to hi, both included. */
export function randInt(lo, hi) {
  return lo + Math.floor(Math.random() * (hi - lo + 1));
}

/** The remainder that is never negative, as the Java pack's scoreboards take it. */
export function mod(a, n) {
  return ((a % n) + n) % n;
}

export function number(v) {
  return typeof v === "number" && Number.isFinite(v) ? v : 0;
}

/** The numbers of a comma-separated list. */
export function numbers(text) {
  return text ? text.split(",").map(Number) : [];
}

let lastWarning = -Infinity;

/** Puts a failure in the content log, at most once a minute. */
export function warn(what, error) {
  if (system.currentTick - lastWarning < 1200) return;
  lastWarning = system.currentTick;
  console.warn(`[Arnis vehicles] ${what}: ${error}`);
}

/** A set of small whole numbers, kept in a world property as hex. */
export class Bits {
  constructor(key, size) {
    this.key = key;
    this.words = new Uint8Array(Math.ceil(size / 4));
    const saved = world.getDynamicProperty(key);
    if (typeof saved === "string") {
      for (let i = 0; i < Math.min(saved.length, this.words.length); i++) {
        this.words[i] = parseInt(saved[i], 16) || 0;
      }
    }
  }

  has(i) {
    return (this.words[i >> 2] & (1 << (i & 3))) !== 0;
  }

  add(i) {
    this.words[i >> 2] |= 1 << (i & 3);
  }

  save() {
    let text = "";
    for (const w of this.words) text += w.toString(16);
    world.setDynamicProperty(this.key, text);
  }
}

/**
 * Moves vehicles the way the Java displays glide between steps: halfway when a step is
 * taken and the rest on the tick after, so they move every tick though they step every
 * second one.
 */
export class Glide {
  constructor() {
    this.pending = new Map();
  }

  move(entity, from, to, yaw) {
    const rotation = { x: 0, y: yaw };
    const target = { x: to.x, y: to.y, z: to.z };
    if (distance(from, to) > 16) {
      // A jump, not a step.
      entity.teleport(target, { rotation });
      this.pending.delete(entity.id);
      return;
    }
    const half = { x: (from.x + to.x) / 2, y: (from.y + to.y) / 2, z: (from.z + to.z) / 2 };
    entity.teleport(half, { rotation });
    this.pending.set(entity.id, { entity, target, rotation });
  }

  finish() {
    for (const { entity, target, rotation } of this.pending.values()) {
      try {
        if (entity.isValid) entity.teleport(target, { rotation });
      } catch (e) {
        warn("moving a vehicle", e);
      }
    }
    this.pending.clear();
  }
}
