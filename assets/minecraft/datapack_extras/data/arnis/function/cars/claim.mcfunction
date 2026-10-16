# A car and its parts are placed on one spot; the id keeps them together once they move.
scoreboard players add #next arnis.car 1
scoreboard players operation @s arnis.car = #next arnis.car
execute as @e[type=block_display,tag=arnis.car.part,distance=..0.05] unless score @s arnis.car matches 1.. run scoreboard players operation @s arnis.car = #next arnis.car
