tag @s add arnis.ds.ent
tag @s add arnis.ds.impact
scoreboard players set #lo arnis.ds 3
scoreboard players set #hi arnis.ds 40
# A quarter of them come down close.
execute store result score #r arnis.ds run random value 1..4
execute if score #r arnis.ds matches 1 run scoreboard players set #hi arnis.ds 9
function arnis:disaster/util/scatter
scoreboard players set @s arnis.ds.t 40
execute store result score #r arnis.ds run random value 1..8
execute if score #r arnis.ds matches 1 run tag @s add arnis.ds.big
tag @s add arnis.ds.aiming
execute at @s run function arnis:disaster/meteor/launch with storage arnis:ds mt
tag @s remove arnis.ds.aiming
