scoreboard players operation @s arnis.ob.run = #run arnis.ob
tag @s remove arnis.ob.turning
data modify entity @s CustomName set from entity @s equipment.head.components."minecraft:custom_name"
attribute @s minecraft:follow_range base set 48
attribute @s minecraft:max_health base set 40
data merge entity @s {Health:40f}
