data remove entity @s interaction
scoreboard players operation #id arnis.car = @s arnis.car
function arnis:cars/seat_kind
execute at @s as @e[type=marker,tag=arnis.car,distance=..0.1] if score @s arnis.car = #id arnis.car run function arnis:cars/seat_take
