# Open water freezes over, with ice that melts again once the sky clears.
execute if block ~ ~-1 ~ minecraft:water[level=0] run return run setblock ~ ~-1 ~ minecraft:frosted_ice
execute if block ~ ~ ~ minecraft:snow[layers=3] run return run function arnis:disaster/snow/drift
execute if block ~ ~ ~ minecraft:snow[layers=2] run return run setblock ~ ~ ~ minecraft:snow[layers=3]
execute if block ~ ~ ~ minecraft:snow[layers=1] run return run setblock ~ ~ ~ minecraft:snow[layers=2]
execute unless block ~ ~ ~ #minecraft:air run return 0
execute if block ~ ~-1 ~ #arnis:no_snow run return 0
execute if block ~ ~-1 ~ #minecraft:slabs[type=bottom] run return 0
setblock ~ ~ ~ minecraft:snow
