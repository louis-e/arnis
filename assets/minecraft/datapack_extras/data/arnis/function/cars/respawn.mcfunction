# Once a second: a car that has driven out of everyone's reach is gone, and new ones join the
# roads around each player, so the traffic there never runs dry.
scoreboard players set #car_r arnis 0
# Who sits in a car, which cars do not stop for.
tag @a[tag=arnis.riding] remove arnis.riding
execute as @e[type=block_display,tag=arnis.car.seat] on passengers run tag @s add arnis.riding
execute as @e[type=marker,tag=arnis.car,scores={arnis.car=1..}] at @s unless entity @a[distance=..168] run function arnis:cars/despawn
execute if score #segs arnis.car matches 1.. as @a at @s run function arnis:cars/respawn_near
