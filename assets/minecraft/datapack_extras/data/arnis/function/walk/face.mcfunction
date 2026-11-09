$execute at @s facing $(x) $(y) $(z) run tp @s ~ ~ ~ ~ ~
# Kept as a score from 0 to 359, read by whoever walks behind; the game keeps turns past 180.
execute store result score @s arnis.wy run data get entity @s Rotation[0]
scoreboard players add @s arnis.wy 3600
scoreboard players operation @s arnis.wy %= #360 arnis.walk
