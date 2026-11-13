execute as @e[type=block_display,tag=arnis.car.seat,distance=..14] if score @s arnis.car = #id arnis.car run kill @s
tag @s remove arnis.car.ridden
