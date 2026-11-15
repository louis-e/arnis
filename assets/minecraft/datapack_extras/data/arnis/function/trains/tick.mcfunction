# A spectator rides the camera of what they clicked, here a vehicle's box: before the
# vehicles move, so they are still where the box is.
execute as @a[gamemode=spectator] at @s if entity @e[type=interaction,tag=arnis.tp,distance=..0.1] run function arnis:trains/watch
scoreboard players add #tt arnis.train 1
execute if score #tt arnis.train matches 2.. run function arnis:trains/drive
