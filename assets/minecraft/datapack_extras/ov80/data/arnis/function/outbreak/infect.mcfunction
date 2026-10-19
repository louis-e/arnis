scoreboard players reset @s arnis.ob.deaths
# Only a death in the mapped area raises a zombie there.
execute unless data entity @s LastDeathLocation{dimension:"minecraft:overworld"} run return 0
execute store result storage arnis:ob death.x int 1 run data get entity @s LastDeathLocation.pos[0]
execute store result storage arnis:ob death.y int 1 run data get entity @s LastDeathLocation.pos[1]
execute store result storage arnis:ob death.z int 1 run data get entity @s LastDeathLocation.pos[2]
execute in minecraft:overworld run function arnis:outbreak/infect_at with storage arnis:ob death
