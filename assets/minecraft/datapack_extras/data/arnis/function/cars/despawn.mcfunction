scoreboard players operation #id arnis.car = @s arnis.car
execute as @e[type=block_display,tag=arnis.car.part,distance=..3] if score @s arnis.car = #id arnis.car run kill @s
kill @s
scoreboard players add #pending arnis.car 1
