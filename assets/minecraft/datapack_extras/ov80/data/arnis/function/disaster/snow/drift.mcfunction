# Now and then a drift deep enough to sink into.
execute store result score #r arnis.ds run random value 1..6
execute if score #r arnis.ds matches 1 run return run setblock ~ ~ ~ minecraft:powder_snow
setblock ~ ~ ~ minecraft:snow[layers=4]
