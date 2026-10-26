execute store result score #n arnis.ds if entity @e[type=block_display,tag=arnis.ds.debris,distance=..40]
execute if score #n arnis.ds matches 18.. run return 0
summon block_display ~ ~ ~ {Tags:["arnis.ds.ent","arnis.ds.debris","arnis.ds.new"],teleport_duration:2,block_state:{Name:"minecraft:oak_leaves"},transformation:{left_rotation:[0.0f,0.0f,0.0f,1.0f],right_rotation:[0.0f,0.0f,0.0f,1.0f],translation:[1.5f,0.3f,0.0f],scale:[0.8f,0.8f,0.8f]}}
execute store result score #r arnis.ds run random value 1..8
execute if score #r arnis.ds matches 2 run data modify entity @e[type=block_display,tag=arnis.ds.new,limit=1] block_state.Name set value "minecraft:birch_leaves"
execute if score #r arnis.ds matches 3 run data modify entity @e[type=block_display,tag=arnis.ds.new,limit=1] block_state.Name set value "minecraft:dirt"
execute if score #r arnis.ds matches 4 run data modify entity @e[type=block_display,tag=arnis.ds.new,limit=1] block_state.Name set value "minecraft:oak_planks"
execute if score #r arnis.ds matches 5 run data modify entity @e[type=block_display,tag=arnis.ds.new,limit=1] block_state.Name set value "minecraft:glass"
execute if score #r arnis.ds matches 6 run data modify entity @e[type=block_display,tag=arnis.ds.new,limit=1] block_state.Name set value "minecraft:cobblestone"
execute if score #r arnis.ds matches 7 run data modify entity @e[type=block_display,tag=arnis.ds.new,limit=1] block_state.Name set value "minecraft:spruce_planks"
execute if score #r arnis.ds matches 8 run data modify entity @e[type=block_display,tag=arnis.ds.new,limit=1] block_state.Name set value "minecraft:bricks"
execute store result entity @e[type=block_display,tag=arnis.ds.new,limit=1] Rotation[0] float 1 run random value 0..359
scoreboard players operation @e[type=block_display,tag=arnis.ds.new] arnis.ds.id = #id arnis.ds
tag @e[type=block_display,tag=arnis.ds.new] remove arnis.ds.new
