# The ground drops whole, with whatever stands on it, but never into the bedrock.
execute store result score @s arnis.ds.d run random value 10..15
execute store result score #room arnis.ds run data get entity @s Pos[1]
scoreboard players operation #room arnis.ds -= #floor arnis.ds
scoreboard players remove #room arnis.ds 1
scoreboard players operation @s arnis.ds.d < #room arnis.ds
execute if score @s arnis.ds.d matches ..2 run return run kill @s
function arnis:disaster/sinkhole/depth
scoreboard players set #ri arnis.ds 0
function arnis:disaster/sinkhole/disc
particle minecraft:block{block_state:"minecraft:dirt"} ~ ~-4 ~ 3 3 3 0 120 force
particle minecraft:campfire_cosy_smoke ~ ~-2 ~ 2 2 2 0.02 30 force
playsound minecraft:entity.generic.explode ambient @a[distance=..64] ~ ~ ~ 2 0.5
playsound minecraft:block.gravel.break ambient @a[distance=..48] ~ ~ ~ 2 0.6
tag @s remove arnis.ds.sink
tag @s add arnis.ds.sinking
scoreboard players set @s arnis.ds.t 25
