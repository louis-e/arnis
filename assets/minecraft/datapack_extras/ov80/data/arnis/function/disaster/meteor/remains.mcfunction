# What came down stays in the crater; now and then it is worth digging out.
execute store result score #r arnis.ds run random value 1..8
execute if score #r arnis.ds matches 1 run setblock ~ ~-1 ~ minecraft:ancient_debris
execute if score #r arnis.ds matches 2.. run setblock ~ ~-1 ~ minecraft:magma_block
particle minecraft:lava ~ ~ ~ 1 0.5 1 0 12 force
particle minecraft:campfire_cosy_smoke ~ ~ ~ 1 0.5 1 0.02 10 force
