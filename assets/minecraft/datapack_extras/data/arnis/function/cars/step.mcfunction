execute unless score @s arnis.car matches 1.. run function arnis:cars/claim
scoreboard players operation #id arnis.car = @s arnis.car
scoreboard players set #red arnis.car 0
# The speed to aim for: the lane's at this driver's pace, held to the bend or junction ahead.
execute store result score #v arnis.car run data get entity @s data.v
scoreboard players operation #t arnis.car = #v arnis.car
scoreboard players operation #q50 arnis.car = #v arnis.car
scoreboard players operation #q50 arnis.car /= #2 arnis
scoreboard players operation #q25 arnis.car = #v arnis.car
scoreboard players operation #q25 arnis.car /= #4 arnis
execute if data entity @s data.m run function arnis:cars/cap
execute if data entity @s data.g run function arnis:cars/light
execute store result score #w arnis.car run data get entity @s data.w
scoreboard players set #ahead arnis.car 0
scoreboard players set #jam arnis.car 0
function arnis:cars/look
execute if score #ahead arnis.car matches 2 run scoreboard players operation #t arnis.car < #q25 arnis.car
execute if score #ahead arnis.car matches 1 run scoreboard players operation #t arnis.car < #q50 arnis.car
# Crossing a junction, a car close ahead is a conflict to give way to for a while; on a
# lane it is a queue to wait in.
execute if score #ahead arnis.car matches 3 unless data entity @s data.j run scoreboard players set #t arnis.car 0
execute if score #ahead arnis.car matches 3 if data entity @s data.j run scoreboard players set #jam arnis.car 1
function arnis:cars/clear
# Gentle on the throttle, firm on the brake.
execute store result score #c arnis.car run data get entity @s data.c
scoreboard players operation #was arnis.car = #c arnis.car
scoreboard players operation #c arnis.car += #accel arnis
scoreboard players operation #c arnis.car < #t arnis.car
execute store result entity @s data.c int 1 run scoreboard players get #c arnis.car
execute store result entity @s data.w int 1 run scoreboard players get #w arnis.car
# Standing for a minute with no red light to blame is stuck: it leaves and another car comes.
execute store result score #q arnis.car run data get entity @s data.q
execute if score #c arnis.car matches 1.. run scoreboard players set #q arnis.car 0
execute if score #c arnis.car matches ..0 unless score #red arnis.car matches 1 run scoreboard players add #q arnis.car 1
execute if score #q arnis.car matches 600.. run return run function arnis:cars/despawn
execute store result entity @s data.q int 1 run scoreboard players get #q arnis.car
function arnis:cars/brake_lights
execute if score #red arnis.car matches 1 if score #c arnis.car matches ..0 run tag @s add arnis.car.held
execute unless score #red arnis.car matches 1 run tag @s remove arnis.car.held
execute if score #c arnis.car matches 1.. run tag @s remove arnis.car.held
execute if score #c arnis.car matches ..0 run return 0
execute store result entity @s data.d double 0.001 run scoreboard players get #c arnis.car
execute as @e[type=block_display,tag=arnis.car.part,distance=..0.1] if score @s arnis.car = #id arnis.car run tag @s add arnis.car.go
function arnis:cars/move with entity @s data
# Still where the move began, so only this car's parts are searched, not every one.
tp @e[type=block_display,tag=arnis.car.go,distance=..0.1] @s
execute at @s run tag @e[type=block_display,tag=arnis.car.go,distance=..0.1] remove arnis.car.go
