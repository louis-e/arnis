scoreboard players set #found arnis.ds 1
execute store result score #kj_gy arnis.ds run data get entity @s Pos[1]
execute at @s run summon warden ~ ~-28 ~ {Tags:["arnis.ds.ent","arnis.ds.kaiju"],NoAI:1b,PersistenceRequired:1b,Invulnerable:1b,CustomName:"Kaiju",attributes:[{id:"minecraft:scale",base:9.0d},{id:"minecraft:knockback_resistance",base:1.0d},{id:"minecraft:waypoint_transmit_range",base:2000.0d}]}
execute as @e[type=warden,tag=arnis.ds.kaiju] run function arnis:disaster/kaiju/setup
kill @s
