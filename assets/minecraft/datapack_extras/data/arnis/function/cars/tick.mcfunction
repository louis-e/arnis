# A spectator rides the camera of what they clicked, here a car's box: before the cars
# move, so they are still where the box is.
execute as @a[gamemode=spectator] at @s if entity @e[type=interaction,tag=arnis.car.part,distance=..0.1] run function arnis:cars/watch
scoreboard players add #car_t arnis 1
execute if score #car_t arnis matches 2.. run function arnis:cars/drive
