scoreboard players remove #gust arnis.ds 1
execute if score #gust arnis.ds matches ..0 run function arnis:disaster/util/wind_gusts
scoreboard players operation #w arnis.ds = #tk arnis.ds
scoreboard players operation #w arnis.ds %= #2 arnis
execute if score #w arnis.ds matches 0 in minecraft:overworld as @a[distance=0..] at @s run function arnis:disaster/util/wind_blow with storage arnis:ds wind
