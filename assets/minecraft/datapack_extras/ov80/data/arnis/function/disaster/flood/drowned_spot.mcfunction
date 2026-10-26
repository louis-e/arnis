tag @s add arnis.ds.ent
scoreboard players set #lo arnis.ds 10
scoreboard players set #hi arnis.ds 28
function arnis:disaster/util/scatter
# Drowned rise only where the flood is deep enough to hide them.
execute at @s if block ~ ~-1 ~ minecraft:light[waterlogged=true] if block ~ ~-2 ~ minecraft:light[waterlogged=true] run summon drowned ~ ~-2 ~ {Tags:["arnis.ds.ent"],PersistenceRequired:1b}
kill @s
