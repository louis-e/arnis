# Leaves that land decay on their own.
execute store result score #r arnis.ds run random value 1..4
execute if score #r arnis.ds matches 1 run summon falling_block ~ ~26 ~ {BlockState:"minecraft:oak_leaves",Time:1,DropItem:0b,HurtEntities:1b,FallHurtAmount:1.0f,FallHurtMax:4,Motion:[0.7d,0.2d,0.3d],Tags:["arnis.ds.ent"]}
execute if score #r arnis.ds matches 2 run summon falling_block ~ ~26 ~ {BlockState:"minecraft:oak_leaves",Time:1,DropItem:0b,HurtEntities:1b,FallHurtAmount:1.0f,FallHurtMax:4,Motion:[-0.6d,0.3d,0.5d],Tags:["arnis.ds.ent"]}
execute if score #r arnis.ds matches 3 run summon falling_block ~ ~26 ~ {BlockState:"minecraft:oak_leaves",Time:1,DropItem:0b,HurtEntities:1b,FallHurtAmount:1.0f,FallHurtMax:4,Motion:[0.4d,0.2d,-0.7d],Tags:["arnis.ds.ent"]}
execute if score #r arnis.ds matches 4 run summon falling_block ~ ~26 ~ {BlockState:"minecraft:oak_leaves",Time:1,DropItem:0b,HurtEntities:1b,FallHurtAmount:1.0f,FallHurtMax:4,Motion:[-0.5d,0.3d,-0.4d],Tags:["arnis.ds.ent"]}
