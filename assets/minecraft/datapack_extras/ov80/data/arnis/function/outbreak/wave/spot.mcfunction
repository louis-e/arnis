execute store result entity @s Rotation[0] float 1 run random value 0..359
# Searching down from a storey above the player lands most spawns in the street, not on roofs.
execute at @s run tp @s ^ ^6 ^26
scoreboard players set #steps arnis 24
scoreboard players set #ok arnis 0
# Snipers take the highest block, which in a city is a roof.
execute if score #kind arnis.ob matches 7 store success score #ok arnis at @s run spreadplayers ~ ~ 0 8 false @s
execute unless score #kind arnis.ob matches 7 run function arnis:util/ground
execute if score #ok arnis matches 1 at @s run function arnis:outbreak/wave/summon
execute if score #ok arnis matches 0 run scoreboard players add #spawn_left arnis.ob 1
kill @s
