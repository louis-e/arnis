scoreboard players set #cd arnis.ds 0
execute store result score #n arnis.ds if entity @e[type=drowned,tag=arnis.ds.ent]
scoreboard players operation #cap arnis.ds = #players arnis.ds
scoreboard players operation #cap arnis.ds *= #4 arnis
scoreboard players add #cap arnis.ds 4
execute if score #n arnis.ds >= #cap arnis.ds run return 0
execute in minecraft:overworld as @a[gamemode=!spectator,distance=0..] at @s summon marker run function arnis:disaster/flood/drowned_spot
