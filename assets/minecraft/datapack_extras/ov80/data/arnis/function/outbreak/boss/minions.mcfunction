scoreboard players set #boss_t arnis.ob 0
scoreboard players set #kind arnis.ob 1
summon zombie ^2 ^ ^1 {Tags:["arnis.ob.ent","arnis.ob.mob","arnis.ob.new"],PersistenceRequired:1b,CanBreakDoors:1b}
summon zombie ^-2 ^ ^1 {Tags:["arnis.ob.ent","arnis.ob.mob","arnis.ob.new"],PersistenceRequired:1b,CanBreakDoors:1b}
execute as @e[tag=arnis.ob.new] run function arnis:outbreak/mob/setup
particle minecraft:large_smoke ~ ~2 ~ 1.5 1 1.5 0.02 60
playsound minecraft:entity.zombie_villager.converted hostile @a ~ ~ ~ 2 0.5
