scoreboard players set #car_t arnis 0
# Only cars someone can see move, so a city of them costs little.
execute as @e[type=marker,tag=arnis.car] at @s if entity @a[distance=..96] run function arnis:cars/step
scoreboard players add #car_r arnis 1
execute if score #car_r arnis matches 10.. run function arnis:cars/respawn
