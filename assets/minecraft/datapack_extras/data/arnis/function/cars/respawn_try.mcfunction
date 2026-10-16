execute store result score #r arnis.car run random value 0..999999
scoreboard players operation #r arnis.car %= #segs arnis.car
execute store result storage arnis:cars spawn.s int 1 run scoreboard players get #r arnis.car
function arnis:cars/respawn_lane with storage arnis:cars spawn
