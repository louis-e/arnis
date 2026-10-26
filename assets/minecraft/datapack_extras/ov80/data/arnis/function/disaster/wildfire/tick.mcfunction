execute as @e[type=marker,tag=arnis.ds.front] at @s run function arnis:disaster/wildfire/front
# Ash in the air everywhere, each player seeing only their own.
scoreboard players operation #m arnis.ds = #tk arnis.ds
scoreboard players operation #m arnis.ds %= #4 arnis
execute if score #m arnis.ds matches 0 in minecraft:overworld as @a[distance=0..] at @s run function arnis:disaster/wildfire/ash
