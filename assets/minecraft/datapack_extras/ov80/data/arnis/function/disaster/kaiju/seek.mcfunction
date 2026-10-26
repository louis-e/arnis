scoreboard players set #lo arnis.ds 60
scoreboard players set #hi arnis.ds 80
function arnis:disaster/util/scatter
# Open ground, so it comes up out of a street or a park and not a roof.
execute store result score #y arnis.ds run data get entity @s Pos[1]
scoreboard players operation #y arnis.ds -= #py arnis.ds
execute if score #y arnis.ds matches -6..4 run return run function arnis:disaster/kaiju/place
kill @s
