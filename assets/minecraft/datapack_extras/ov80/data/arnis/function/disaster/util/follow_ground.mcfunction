# Keeps the executing entity on the terrain: it follows the ground up and down a few blocks
# at a time, and passes through anything taller, which is a building and not a hill.
execute positioned over ocean_floor summon marker run function arnis:disaster/util/probe
execute store result score #cy arnis.ds run data get entity @s Pos[1]
scoreboard players operation #gy arnis.ds -= #cy arnis.ds
execute if score #gy arnis.ds matches 3.. run return 0
execute if score #gy arnis.ds matches ..-4 run scoreboard players set #gy arnis.ds -3
scoreboard players operation #gy arnis.ds += #cy arnis.ds
execute store result entity @s Pos[1] double 1 run scoreboard players get #gy arnis.ds
