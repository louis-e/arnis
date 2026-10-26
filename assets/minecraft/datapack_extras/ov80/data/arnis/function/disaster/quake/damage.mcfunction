tag @s add arnis.ds.ent
scoreboard players set #lo arnis.ds 6
scoreboard players set #hi arnis.ds 44
function arnis:disaster/util/scatter
execute store result score #y arnis.ds run data get entity @s Pos[1]
scoreboard players operation #y arnis.ds -= #py arnis.ds
# Up on a roof, or down in the street.
scoreboard players set #roof arnis.ds 0
execute if score #y arnis.ds matches 4..90 run scoreboard players set #roof arnis.ds 1
execute store result score #r arnis.ds run random value 1..10
execute if score #r arnis.ds matches 1..3 at @s run function arnis:disaster/quake/shatter
execute if score #r arnis.ds matches 4..5 at @s run function arnis:disaster/quake/crack
execute if score #r arnis.ds matches 6..8 if score #roof arnis.ds matches 1 at @s run function arnis:disaster/quake/sag
execute if score #r arnis.ds matches 6..8 if score #roof arnis.ds matches 0 at @s run function arnis:disaster/quake/shatter
execute if score #r arnis.ds matches 9..10 if score #roof arnis.ds matches 1 at @s run function arnis:disaster/quake/cave_in
execute if score #r arnis.ds matches 9..10 if score #roof arnis.ds matches 0 at @s run function arnis:disaster/quake/crack
kill @s
