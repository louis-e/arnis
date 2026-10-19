scoreboard players operation @s arnis.ob.run = #run arnis.ob
tag @s remove arnis.ob.new
attribute @s minecraft:follow_range base set 40
execute if score #kind arnis.ob matches 3 run attribute @s minecraft:movement_speed base set 0.33
execute store result score #r2 arnis.ob run random value 1..3
execute if score #kind arnis.ob matches 6 if score #r2 arnis.ob matches 1 run item replace entity @s weapon.mainhand with minecraft:trident
# Summoned with NBT, so it skipped the spawn equipment a skeleton draws its bow from.
execute if score #kind arnis.ob matches 7 run item replace entity @s weapon.mainhand with minecraft:bow
execute if score #kind arnis.ob matches 8 run function arnis:outbreak/mob/brute
