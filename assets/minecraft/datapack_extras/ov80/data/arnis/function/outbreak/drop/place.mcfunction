execute store result entity @s Rotation[0] float 1 run random value 0..359
execute at @s run tp @s ^ ^12 ^45
scoreboard players set #steps arnis 32
function arnis:util/ground
execute if score #ok arnis matches 1 at @s run function arnis:outbreak/drop/land
kill @s
