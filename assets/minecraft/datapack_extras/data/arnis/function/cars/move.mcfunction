# A target in a chunk that is not loaded waits, rather than the car leaving the world it can be seen in.
$execute positioned $(x) $(y) $(z) unless loaded ~ ~ ~ run return 0
$execute if entity @s[x=$(x),y=$(y),z=$(z),distance=..$(d)] run return run function arnis:cars/arrive with entity @s data
$execute facing $(x) $(y) $(z) run tp @s ^ ^ ^$(d) ~ 0
