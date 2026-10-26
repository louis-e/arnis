tag @s add arnis.ds.ent
scoreboard players set #lo arnis.ds 5
scoreboard players set #hi arnis.ds 26
function arnis:disaster/util/scatter
# Open ground about level with the player, not a roof.
execute store result score #y arnis.ds run data get entity @s Pos[1]
scoreboard players operation #y arnis.ds -= #py arnis.ds
execute if score #y arnis.ds matches -3..3 run return run function arnis:disaster/sinkhole/arm
kill @s
