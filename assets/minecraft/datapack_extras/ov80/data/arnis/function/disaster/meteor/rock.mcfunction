tag @s add arnis.ds.ent
tag @s add arnis.ds.rock
scoreboard players set @s arnis.ds.t 40
execute facing entity @e[type=marker,tag=arnis.ds.aiming,limit=1] feet run tp @s ~ ~ ~ ~ ~
data merge entity @s {block_state:{Name:"minecraft:magma_block"},teleport_duration:2,brightness:{sky:15,block:15},transformation:{left_rotation:[0.0f,0.0f,0.0f,1.0f],right_rotation:[0.0f,0.0f,0.0f,1.0f],translation:[-1.0f,-1.0f,-1.0f],scale:[2.0f,2.0f,2.0f]}}
execute if entity @e[type=marker,tag=arnis.ds.aiming,tag=arnis.ds.big] run data merge entity @s {transformation:{left_rotation:[0.0f,0.0f,0.0f,1.0f],right_rotation:[0.0f,0.0f,0.0f,1.0f],translation:[-1.75f,-1.75f,-1.75f],scale:[3.5f,3.5f,3.5f]}}
