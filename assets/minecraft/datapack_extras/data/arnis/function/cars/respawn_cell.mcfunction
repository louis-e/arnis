$execute store result score #n arnis.car run data get storage arnis:cars grid."$(cx)_$(cz)"
execute if score #n arnis.car matches ..0 run return 0
# One chance per spot, so a busy road fills up faster than a quiet one.
execute store result score #chance arnis.car run random value 0..7
execute if score #chance arnis.car >= #n arnis.car run return 0
execute store result score #r arnis.car run random value 0..999999
scoreboard players operation #r arnis.car %= #n arnis.car
execute store result storage arnis:cars spawn.k int 1 run scoreboard players get #r arnis.car
function arnis:cars/respawn_spot with storage arnis:cars spawn
