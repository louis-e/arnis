setblock ~ ~ ~ minecraft:chest{LootTable:"arnis:outbreak/supply_drop"}
summon marker ~ ~ ~ {Tags:["arnis.ob.ent","arnis.ob.drop"]}
# Hidden in the ground under the chest: the locator bar only tracks living entities.
summon armor_stand ~ ~-1.5 ~ {Tags:["arnis.ob.ent","arnis.ob.drop_wp"],Marker:1b,NoGravity:1b,Invulnerable:1b,Silent:1b,attributes:[{id:"minecraft:waypoint_transmit_range",base:2000.0d}]}
waypoint modify @e[type=armor_stand,tag=arnis.ob.drop_wp,distance=..3,limit=1,sort=nearest] color gold
execute store result score #dx arnis.ob run data get entity @s Pos[0]
execute store result score #dy arnis.ob run data get entity @s Pos[1]
execute store result score #dz arnis.ob run data get entity @s Pos[2]
tellraw @a ["",{"text":"Supply drop at ","color":"gold"},{"score":{"name":"#dx","objective":"arnis.ob"},"color":"yellow"},{"text":" "},{"score":{"name":"#dy","objective":"arnis.ob"},"color":"yellow"},{"text":" "},{"score":{"name":"#dz","objective":"arnis.ob"},"color":"yellow"},{"text":" (follow the smoke)","color":"gray"}]
execute as @a at @s run playsound minecraft:entity.firework_rocket.launch master @s ~ ~ ~ 1 0.7
