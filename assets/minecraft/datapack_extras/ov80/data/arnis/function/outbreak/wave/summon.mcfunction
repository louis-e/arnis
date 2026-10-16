execute if score #kind arnis.ob matches 1 run summon zombie ~ ~ ~ {Tags:["arnis.ob.ent","arnis.ob.mob","arnis.ob.new"],PersistenceRequired:1b,CanBreakDoors:1b}
execute if score #kind arnis.ob matches 2 run summon zombie_villager ~ ~ ~ {Tags:["arnis.ob.ent","arnis.ob.mob","arnis.ob.new"],PersistenceRequired:1b,CanBreakDoors:1b}
execute if score #kind arnis.ob matches 3 run summon zombie ~ ~ ~ {Tags:["arnis.ob.ent","arnis.ob.mob","arnis.ob.new"],PersistenceRequired:1b,CanBreakDoors:1b,CustomName:{"text":"Runner","color":"yellow"}}
execute if score #kind arnis.ob matches 4 run summon zombie ~ ~ ~ {Tags:["arnis.ob.ent","arnis.ob.mob","arnis.ob.new"],PersistenceRequired:1b,CanBreakDoors:1b,IsBaby:1b}
execute if score #kind arnis.ob matches 5 run summon husk ~ ~ ~ {Tags:["arnis.ob.ent","arnis.ob.mob","arnis.ob.new"],PersistenceRequired:1b,CanBreakDoors:1b}
execute if score #kind arnis.ob matches 6 run summon drowned ~ ~ ~ {Tags:["arnis.ob.ent","arnis.ob.mob","arnis.ob.new"],PersistenceRequired:1b}
execute if score #kind arnis.ob matches 7 run summon skeleton ~ ~ ~ {Tags:["arnis.ob.ent","arnis.ob.mob","arnis.ob.new"],PersistenceRequired:1b}
execute if score #kind arnis.ob matches 8 run summon zombie ~ ~ ~ {Tags:["arnis.ob.ent","arnis.ob.mob","arnis.ob.new"],PersistenceRequired:1b,CanBreakDoors:1b,CustomName:{"text":"Brute","color":"dark_red"}}
execute if score #kind arnis.ob matches 9 run summon creeper ~ ~ ~ {Tags:["arnis.ob.ent","arnis.ob.mob","arnis.ob.new"],PersistenceRequired:1b}
execute if score #kind arnis.ob matches 10 run summon spider ~ ~ ~ {Tags:["arnis.ob.ent","arnis.ob.mob","arnis.ob.new"],PersistenceRequired:1b}
execute as @e[tag=arnis.ob.new] run function arnis:outbreak/mob/setup
