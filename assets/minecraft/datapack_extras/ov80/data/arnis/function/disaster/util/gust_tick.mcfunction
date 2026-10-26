scoreboard players remove @s arnis.ds.t 1
execute if score @s arnis.ds.t matches 1.. run return 0
execute if block ~ ~ ~ minecraft:barrier run setblock ~ ~ ~ minecraft:air
kill @s
