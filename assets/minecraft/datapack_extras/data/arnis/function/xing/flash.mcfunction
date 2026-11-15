# The two lamps of every closed crossing's masts light in turn, and its bell strikes with them.
scoreboard players add #fl arnis.xing 1
execute if score #fl arnis.xing matches 2.. run scoreboard players set #fl arnis.xing 0
execute if score #fl arnis.xing matches 0 as @e[type=item_display,tag=arnis.xon] run data modify entity @s item.components."minecraft:item_model" set value "arnis:vehicle/xing_light_a_std"
execute if score #fl arnis.xing matches 1 as @e[type=item_display,tag=arnis.xon] run data modify entity @s item.components."minecraft:item_model" set value "arnis:vehicle/xing_light_b_std"
execute if score #fl arnis.xing matches 0 at @e[type=item_display,tag=arnis.xring] run playsound minecraft:block.note_block.bell block @a[distance=..40] ~ ~ ~ 1.2 1.26
execute if score #fl arnis.xing matches 1 at @e[type=item_display,tag=arnis.xring] run playsound minecraft:block.note_block.bell block @a[distance=..40] ~ ~ ~ 1.2 1.19
