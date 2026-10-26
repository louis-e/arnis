scoreboard players operation #qk_next arnis.ds = #t arnis.ds
scoreboard players add #qk_next arnis.ds 4
execute if score #pass arnis.ds matches 2.. run scoreboard players remove #qk_next arnis.ds 1
execute store result score #n arnis.ds if entity @e[type=marker,tag=arnis.ds.fissure]
execute if score #n arnis.ds matches 16.. run return 0
execute in minecraft:overworld as @a[gamemode=!spectator,distance=0..] at @s run function arnis:disaster/quake/fissure
