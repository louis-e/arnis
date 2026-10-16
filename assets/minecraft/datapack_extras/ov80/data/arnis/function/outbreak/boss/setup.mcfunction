tag @s remove arnis.ob.new
attribute @s minecraft:scale base set 3
attribute @s minecraft:max_health base set 400
data merge entity @s {Health:400f}
attribute @s minecraft:attack_damage base set 14
attribute @s minecraft:movement_speed base set 0.26
attribute @s minecraft:knockback_resistance base set 1
attribute @s minecraft:follow_range base set 64
attribute @s minecraft:armor base set 8
attribute @s minecraft:fall_damage_multiplier base set 0
effect give @s minecraft:fire_resistance infinite 0 true
item replace entity @s weapon.mainhand with minecraft:iron_axe
bossbar set arnis:boss value 400
bossbar set arnis:boss visible true
