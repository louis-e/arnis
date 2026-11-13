$execute store result score #n arnis.walk run data get storage arnis:walk grid."$(cx)_$(cz)"
execute if score #n arnis.walk matches ..0 run return 0
# One chance per spot, so a busy street fills up faster than a quiet path.
execute store result score #chance arnis.walk run random value 0..11
execute if score #chance arnis.walk >= #n arnis.walk run return 0
execute store result score #r arnis.walk run random value 0..999999
scoreboard players operation #r arnis.walk %= #n arnis.walk
execute store result storage arnis:walk spawn.k int 1 run scoreboard players get #r arnis.walk
function arnis:walk/spot with storage arnis:walk spawn
