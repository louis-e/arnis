# The seat sits ahead of the car's middle, so it is not among the parts that follow it.
function arnis:cars/seat_check
execute if score #rider arnis.car matches 0 run return 0
$execute rotated as @s as @e[type=block_display,tag=arnis.car.seat,distance=..14] if score @s arnis.car = #id arnis.car run tp @s ^ ^$(u) ^$(f)
