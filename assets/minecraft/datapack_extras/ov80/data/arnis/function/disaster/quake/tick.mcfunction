scoreboard players add #qk arnis.ds 1
scoreboard players operation #q4 arnis.ds = #qk arnis.ds
scoreboard players operation #q4 arnis.ds %= #4 arnis
# The last five seconds are the aftershock.
scoreboard players operation #q arnis.ds = #len arnis.ds
scoreboard players operation #q arnis.ds -= #t arnis.ds
execute if score #q arnis.ds matches 6.. in minecraft:overworld as @a[gamemode=!spectator,distance=0..] at @s run function arnis:disaster/quake/shake
execute if score #q arnis.ds matches ..5 in minecraft:overworld as @a[gamemode=!spectator,distance=0..] at @s run function arnis:disaster/quake/tremor
execute as @e[type=marker,tag=arnis.ds.fissure] at @s run function arnis:disaster/quake/split
