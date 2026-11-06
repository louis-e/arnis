execute if score #near arnis.car >= #near_cap arnis.car run return 0
# A random map cell around the player, no further than reach.
execute store result score #cx arnis.car run random value -3..3
execute store result score #cz arnis.car run random value -3..3
scoreboard players operation #cx arnis.car += #px arnis.car
scoreboard players operation #cz arnis.car += #pz arnis.car
execute store result storage arnis:cars spawn.cx int 1 run scoreboard players get #cx arnis.car
execute store result storage arnis:cars spawn.cz int 1 run scoreboard players get #cz arnis.car
function arnis:cars/respawn_cell with storage arnis:cars spawn
scoreboard players remove #tries arnis.car 1
execute if score #tries arnis.car matches 1.. run function arnis:cars/respawn_try
