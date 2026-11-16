tp @s ~ ~ ~ ~ ~
scoreboard players operation @s arnis.te = #now arnis.train
scoreboard players add #found arnis.train 1
# Wheels up or down, when the point says otherwise than the model shows.
$execute unless score @s arnis.tg matches $(g) run function arnis:trains/air_gear$(g) with storage arnis:trains c
