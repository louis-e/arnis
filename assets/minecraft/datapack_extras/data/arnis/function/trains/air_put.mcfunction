# At point i and a fraction f of a block on, the way the path runs there (yaw h, pitch q),
# the nose as far up as the point says (pitch p). The model stands a block up along its own
# up, lit by the air rather than the ground it cuts into on a slope, and turns about the
# main wheels all the same.
$execute positioned $(x) $(y) $(z) unless loaded ~ ~ ~ run return 0
scoreboard players set #found arnis.train 0
$execute positioned $(x) $(y) $(z) rotated $(h) $(q) positioned ^ ^ ^$(f) rotated $(h) $(p) positioned ^ ^1 ^ as @e[type=item_display,tag=arnis.tc$(c),distance=..12] run function arnis:trains/air_part with storage arnis:trains c
# Not there yet: it has just come into view.
$execute if score #found arnis.train matches 0 positioned $(x) $(y) $(z) rotated $(h) $(q) positioned ^ ^ ^$(f) rotated $(h) $(p) positioned ^ ^1 ^ run function arnis:trains/body/$(b) with storage arnis:trains c
$execute if score #found arnis.train matches 0 positioned $(x) $(y) $(z) rotated $(h) $(q) positioned ^ ^ ^$(f) rotated $(h) $(p) positioned ^ ^1 ^ as @e[type=item_display,tag=arnis.tc$(c),distance=..0.5] run scoreboard players operation @s arnis.te = #now arnis.train
# Places to get on, only where someone could reach them; they pitch with the cabin.
$execute positioned $(x) $(y) $(z) rotated $(h) $(q) positioned ^ ^ ^$(f) rotated $(h) $(p) if entity @a[distance=..64] run function arnis:trains/door/$(d) with storage arnis:trains c
