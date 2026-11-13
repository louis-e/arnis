execute store result score #near arnis.car if entity @e[type=marker,tag=arnis.car,distance=..168]
execute if score #near arnis.car >= #near_cap arnis.car run return 0
execute store result score #px arnis.car run data get entity @s Pos[0]
execute store result score #pz arnis.car run data get entity @s Pos[2]
scoreboard players operation #px arnis.car /= #spot_cell arnis.car
scoreboard players operation #pz arnis.car /= #spot_cell arnis.car
scoreboard players set #far arnis.car 0
execute positioned ~150 ~ ~ if loaded ~ ~ ~ positioned ~-300 ~ ~ if loaded ~ ~ ~ run scoreboard players set #far arnis.car 1
scoreboard players set #tries arnis.car 56
function arnis:cars/respawn_try
