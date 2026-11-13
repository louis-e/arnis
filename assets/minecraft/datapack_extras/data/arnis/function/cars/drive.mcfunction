scoreboard players set #car_t arnis 0
# Display entities are sent out to 160 blocks, so every car anyone can see is driving.
execute as @e[type=marker,tag=arnis.car] at @s if entity @a[distance=..168] run function arnis:cars/step
scoreboard players add #car_r arnis 1
execute if score #car_r arnis matches 10.. run function arnis:cars/respawn
