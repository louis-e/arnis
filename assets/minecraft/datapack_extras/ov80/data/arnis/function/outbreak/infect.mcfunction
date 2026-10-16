scoreboard players reset @s arnis.ob.deaths
execute store result storage arnis:ob death.x int 1 run data get entity @s LastDeathLocation.pos[0]
execute store result storage arnis:ob death.y int 1 run data get entity @s LastDeathLocation.pos[1]
execute store result storage arnis:ob death.z int 1 run data get entity @s LastDeathLocation.pos[2]
function arnis:outbreak/infect_at with storage arnis:ob death
