scoreboard players add #qk arnis.ds 1
scoreboard players operation #q arnis.ds = #qk arnis.ds
scoreboard players operation #q arnis.ds %= #60 arnis
execute if score #q arnis.ds matches 0 in minecraft:overworld as @a[distance=0..] at @s run playsound minecraft:entity.warden.dig ambient @s ~ ~-3 ~ 1 0.5
execute if score #q arnis.ds matches ..7 in minecraft:overworld as @a[gamemode=!spectator,distance=0..] at @s run function arnis:disaster/quake/tremor
