function arnis:disaster/util/wind_tick
scoreboard players operation #m arnis.ds = #tk arnis.ds
scoreboard players operation #m arnis.ds %= #4 arnis
execute if score #m arnis.ds matches 0 in minecraft:overworld as @a[distance=0..] at @s run function arnis:disaster/snow/flurry
