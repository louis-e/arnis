# A spot is the lane and the point on it the car starts from.
$data modify storage arnis:cars spawn.s set from storage arnis:cars grid."$(cx)_$(cz)"[$(k)][0]
$data modify storage arnis:cars spawn.a set from storage arnis:cars grid."$(cx)_$(cz)"[$(k)][1]
execute store result score #i arnis.car run data get storage arnis:cars spawn.a
scoreboard players add #i arnis.car 1
execute store result storage arnis:cars spawn.i int 1 run scoreboard players get #i arnis.car
function arnis:cars/respawn_lane with storage arnis:cars spawn
