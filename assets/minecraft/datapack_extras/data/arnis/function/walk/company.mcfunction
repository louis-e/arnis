# A step behind on the same way, at the same pace and with the same chances, so the two
# keep together through every turn and stop.
execute store result storage arnis:walk spawn.l int 1 run random value 0..17
execute store result storage arnis:walk spawn.h int 1 run random value 0..7
execute if score #company arnis.walk matches 2 store result storage arnis:walk spawn.h int 1 run random value 8..9
function arnis:walk/look with storage arnis:walk spawn
function arnis:walk/dress
function arnis:walk/body with storage arnis:walk spawn
execute as @e[tag=arnis.walk_new] at @s run function arnis:walk/along
scoreboard players add #near arnis.walk 1
