tag @s add arnis.ds.ent
tag @s add arnis.ds.fissure
scoreboard players set #lo arnis.ds 4
scoreboard players set #hi arnis.ds 20
function arnis:disaster/util/scatter
execute store result entity @s Pos[1] double 1 run scoreboard players get #py arnis.ds
execute store result entity @s Rotation[0] float 1 run random value 0..359
execute store result score @s arnis.ds.t run random value 40..70
scoreboard players set @s arnis.ds.id 1
