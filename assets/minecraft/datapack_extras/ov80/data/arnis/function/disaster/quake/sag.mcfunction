# A part of the building drops onto the storey below.
execute store result score #h arnis.ds run random value 3..5
execute store result storage arnis:ds sag.w int 1 run random value 4..6
scoreboard players set #sb arnis.ds 3
scoreboard players operation #sb arnis.ds -= #y arnis.ds
execute store result storage arnis:ds sag.sb int 1 run scoreboard players get #sb arnis.ds
scoreboard players operation #sb arnis.ds -= #h arnis.ds
execute store result storage arnis:ds sag.db int 1 run scoreboard players get #sb arnis.ds
execute store result storage arnis:ds sag.h1 int 1 run scoreboard players remove #h arnis.ds 1
function arnis:disaster/quake/sag_at with storage arnis:ds sag
function arnis:disaster/quake/debris
particle minecraft:explosion ~ ~-4 ~ 3 3 3 0 4
particle minecraft:block{block_state:"minecraft:stone_bricks"} ~ ~-4 ~ 4 4 4 0 60
particle minecraft:campfire_cosy_smoke ~ ~-2 ~ 3 2 3 0.02 8
playsound minecraft:entity.generic.explode block @a[distance=..64] ~ ~ ~ 2 0.5
playsound minecraft:block.stone.break block @a[distance=..48] ~ ~ ~ 2 0.6
