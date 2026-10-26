tag @s add arnis.ds.ent
tag @s add arnis.ds.bolt_try
scoreboard players set #lo arnis.ds 4
scoreboard players set #hi arnis.ds 36
function arnis:disaster/util/scatter
execute store result score @s arnis.ds.t run data get entity @s Pos[1]
scoreboard players operation #best arnis.ds > @s arnis.ds.t
