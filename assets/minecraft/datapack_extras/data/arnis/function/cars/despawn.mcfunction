scoreboard players operation #id arnis.car = @s arnis.car
execute as @e[tag=arnis.car.part,distance=..14] if score @s arnis.car = #id arnis.car run kill @s
kill @s
