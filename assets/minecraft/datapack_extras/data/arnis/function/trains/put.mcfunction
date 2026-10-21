# Between point i and the next, a fraction f of the way, facing along the track.
$execute positioned $(x) $(y) $(z) unless loaded ~ ~ ~ run return 0
scoreboard players set #found arnis.train 0
$execute positioned $(x) $(y) $(z) rotated $(h) 0 positioned ^ ^ ^$(f) as @e[type=block_display,tag=arnis.tc$(c),distance=..12] run function arnis:trains/part
# Not there yet: it has just come into view, or into the area.
$execute if score #found arnis.train matches 0 positioned $(x) $(y) $(z) rotated $(h) 0 positioned ^ ^ ^$(f) run function arnis:trains/body/$(b) with storage arnis:trains c
$execute if score #found arnis.train matches 0 positioned $(x) $(y) $(z) rotated $(h) 0 positioned ^ ^ ^$(f) as @e[type=block_display,tag=arnis.tc$(c),distance=..0.5] run scoreboard players operation @s arnis.te = #now arnis.train
