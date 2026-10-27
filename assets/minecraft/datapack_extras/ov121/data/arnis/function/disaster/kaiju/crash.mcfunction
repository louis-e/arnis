particle minecraft:explosion ~ ~8 ~ 4 6 4 0 3 force
particle minecraft:block{block_state:"minecraft:stone_bricks"} ~ ~8 ~ 4 6 4 0 40 force
particle minecraft:campfire_cosy_smoke ~ ~4 ~ 4 3 4 0.02 6 force
playsound minecraft:entity.generic.explode hostile @a[distance=..96] ~ ~6 ~ 3 0.6
playsound minecraft:block.stone.break hostile @a[distance=..64] ~ ~6 ~ 3 0.5
# Rubble flies off and stays where it lands.
execute store result score #r arnis.ds run random value 1..6
execute if score #r arnis.ds matches 1 run summon falling_block ~ ~14 ~ {BlockState:"minecraft:cobblestone",Time:1,DropItem:0b,HurtEntities:1b,FallHurtAmount:2.0f,FallHurtMax:8,Motion:[0.5d,0.4d,0.3d],Tags:["arnis.ds.ent"]}
execute if score #r arnis.ds matches 2 run summon falling_block ~ ~14 ~ {BlockState:"minecraft:cobblestone",Time:1,DropItem:0b,HurtEntities:1b,FallHurtAmount:2.0f,FallHurtMax:8,Motion:[-0.4d,0.5d,0.4d],Tags:["arnis.ds.ent"]}
execute if score #r arnis.ds matches 3 run summon falling_block ~ ~14 ~ {BlockState:"minecraft:gravel",Time:1,DropItem:0b,HurtEntities:1b,FallHurtAmount:2.0f,FallHurtMax:8,Motion:[0.3d,0.5d,-0.5d],Tags:["arnis.ds.ent"]}
