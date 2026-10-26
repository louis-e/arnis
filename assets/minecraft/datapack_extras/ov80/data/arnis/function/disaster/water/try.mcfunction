# A fill fails whole on a corner that is not loaded, so the cell waits until all of it is.
# A block test fails where a fill would, where `if loaded` would want entity ticking too.
scoreboard players set #ok arnis.ds 0
$execute unless block $(x0) 0 $(z0) minecraft:structure_void run scoreboard players add #ok arnis.ds 1
$execute unless block $(x1) 0 $(z0) minecraft:structure_void run scoreboard players add #ok arnis.ds 1
$execute unless block $(x0) 0 $(z1) minecraft:structure_void run scoreboard players add #ok arnis.ds 1
$execute unless block $(x1) 0 $(z1) minecraft:structure_void run scoreboard players add #ok arnis.ds 1
execute unless score #ok arnis.ds matches 4 run return 0
# A drain can wake water that spilled; it must not turn into sources that stay.
$execute if score #c$(x)_$(z) arnis.ds.$(map) > #lv_$(map) arnis.ds run function arnis:disaster/water/hold
$execute if score #c$(x)_$(z) arnis.ds.$(map) > #lv_$(map) arnis.ds run function arnis:disaster/$(map)/d/$(x)_$(z) with storage arnis:ds $(map).t
$execute unless score #c$(x)_$(z) arnis.ds.$(map) >= #lv_$(map) arnis.ds run function arnis:disaster/$(map)/c/$(x)_$(z) with storage arnis:ds $(map).t
$scoreboard players operation #c$(x)_$(z) arnis.ds.$(map) = #lv_$(map) arnis.ds
scoreboard players remove #budget arnis.ds 1
