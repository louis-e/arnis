# A front creeps toward the nearest player, slower than they walk.
execute facing entity @p[gamemode=!spectator] feet rotated ~ 0 run tp @s ^ ^ ^0.09
particle minecraft:campfire_signal_smoke ~ ~2 ~ 3 1 3 0.01 2 force
particle minecraft:large_smoke ~ ~1 ~ 4 1 4 0.02 4 force
particle minecraft:flame ~ ~0.5 ~ 4 0.6 4 0.02 7 force
particle minecraft:lava ~ ~0.5 ~ 4 0.4 4 0 1 force
scoreboard players add @s arnis.ds.t 1
scoreboard players operation #m arnis.ds = @s arnis.ds.t
scoreboard players operation #m arnis.ds %= #10 arnis
execute if score #m arnis.ds matches 0 run function arnis:disaster/wildfire/burn
scoreboard players operation #m arnis.ds = @s arnis.ds.t
scoreboard players operation #m arnis.ds %= #20 arnis
execute if score #m arnis.ds matches 0 run function arnis:disaster/wildfire/embers
