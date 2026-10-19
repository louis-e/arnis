execute at @s if block ~ ~ ~ #minecraft:replaceable if block ~ ~1 ~ #minecraft:replaceable unless block ~ ~-1 ~ #minecraft:replaceable run return run scoreboard players set #ok arnis 1
scoreboard players remove #steps arnis 1
execute if score #steps arnis matches 1.. at @s run tp @s ~ ~-1 ~
execute if score #steps arnis matches 1.. run function arnis:util/ground_step
