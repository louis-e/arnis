# Everyone got far enough away that it was left behind, unloaded: it forms again near them,
# and the old one goes the next time it loads.
scoreboard players operation #tw_min arnis.ds = #tw_id arnis.ds
scoreboard players add #tw_min arnis.ds 1
execute in minecraft:overworld as @r[gamemode=!spectator,distance=0..] at @s run function arnis:disaster/tornado/spawn
execute if score #pass arnis.ds matches 2.. in minecraft:overworld as @r[gamemode=!spectator,distance=0..] at @s run function arnis:disaster/tornado/spawn
