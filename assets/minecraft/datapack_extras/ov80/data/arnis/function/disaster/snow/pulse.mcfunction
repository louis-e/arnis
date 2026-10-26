scoreboard players set #engine arnis.ds 1
data modify storage arnis:ds scan.map set value "snow"
scoreboard players set #budget arnis.ds 1
execute in minecraft:overworld as @a[distance=0..] at @s run function arnis:disaster/water/around
execute if score #budget arnis.ds matches 1 run scoreboard players set #wd_snow arnis.ds 0
scoreboard players set #engine arnis.ds 0
