execute store result entity @s data.b byte 1 run scoreboard players get #b arnis.car
execute if score #b arnis.car matches 1 as @e[type=block_display,tag=arnis.car.tail,distance=..0.1] if score @s arnis.car = #id arnis.car run data merge entity @s {brightness:{sky:15,block:15}}
execute if score #b arnis.car matches 0 as @e[type=block_display,tag=arnis.car.tail,distance=..0.1] if score @s arnis.car = #id arnis.car run data merge entity @s {brightness:{sky:7,block:7}}
