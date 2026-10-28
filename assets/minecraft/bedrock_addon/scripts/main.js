// Drives the world's cars and trains. The vehicles step every second tick, as in the Java
// pack, and glide the rest of the way on the tick between.
import { GameMode, system, world } from "@minecraft/server";
import { Cars } from "./cars.js";
import { DATA } from "./data.js";
import { Trains } from "./trains.js";
import { Glide, warn } from "./util.js";

let started = false;

function start() {
  if (started) return;
  let dimension;
  try {
    dimension = world.getDimension("overworld");
  } catch {
    // Not loaded yet; the world load event comes next.
    return;
  }
  started = true;
  const glide = new Glide();
  const cars = DATA.cars ? new Cars(dimension, DATA.cars, glide) : undefined;
  const trains = DATA.trains ? new Trains(dimension, DATA.trains, glide) : undefined;
  let tick = 0;
  system.runInterval(() => {
    tick++;
    if (tick % 2 === 1) {
      glide.finish();
      return;
    }
    let players = [];
    try {
      players = dimension.getPlayers().map((p) => ({
        location: p.location,
        walks: p.getGameMode() !== GameMode.Spectator,
      }));
    } catch (e) {
      // The trains' clock runs on all the same.
      warn("finding players", e);
    }
    if (cars && players.length > 0) {
      try {
        cars.step(players);
      } catch (e) {
        warn("cars", e);
      }
    }
    if (trains) {
      try {
        trains.step(players);
      } catch (e) {
        warn("trains", e);
      }
    }
  }, 1);
}

world.afterEvents.worldLoad.subscribe(start);
system.run(start);
