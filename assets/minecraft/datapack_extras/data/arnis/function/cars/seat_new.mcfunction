$execute rotated as @s run summon block_display ^ ^$(u) ^$(f) {Tags:["arnis.car.part","arnis.car.seat","arnis.car.new_seat"],teleport_duration:2}
execute as @e[type=block_display,tag=arnis.car.new_seat,distance=..14] run scoreboard players operation @s arnis.car = #id arnis.car
ride @p[tag=arnis.boarding] mount @e[type=block_display,tag=arnis.car.new_seat,distance=..14,limit=1]
execute as @e[type=block_display,tag=arnis.car.new_seat,distance=..14] on passengers run tag @s add arnis.riding
tag @e[type=block_display,tag=arnis.car.new_seat,distance=..14] remove arnis.car.new_seat
tag @s add arnis.car.ridden
