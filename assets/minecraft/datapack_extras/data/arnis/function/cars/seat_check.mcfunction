# Frees the seat of a car nobody sits in any more.
scoreboard players set #rider arnis.car 0
execute as @e[type=block_display,tag=arnis.car.seat,distance=..14] if score @s arnis.car = #id arnis.car on passengers run scoreboard players set #rider arnis.car 1
execute if score #rider arnis.car matches 0 run function arnis:cars/seat_free
