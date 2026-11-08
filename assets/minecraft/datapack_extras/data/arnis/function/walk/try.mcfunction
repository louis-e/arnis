execute if score #near arnis.walk >= #cap arnis.walk run return 0
# A random map cell around the player.
execute store result score #cx arnis.walk run random value -2..2
execute store result score #cz arnis.walk run random value -2..2
scoreboard players operation #cx arnis.walk += #px arnis.walk
scoreboard players operation #cz arnis.walk += #pz arnis.walk
execute store result storage arnis:walk spawn.cx int 1 run scoreboard players get #cx arnis.walk
execute store result storage arnis:walk spawn.cz int 1 run scoreboard players get #cz arnis.walk
function arnis:walk/cell with storage arnis:walk spawn
scoreboard players remove #tries arnis.walk 1
execute if score #tries arnis.walk matches 1.. run function arnis:walk/try
