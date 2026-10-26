# Like the water: a cell changes only once all of it is loaded.
scoreboard players set #ok arnis.ds 0
$execute unless block $(x0) 0 $(z0) minecraft:structure_void run scoreboard players add #ok arnis.ds 1
$execute unless block $(x1) 0 $(z0) minecraft:structure_void run scoreboard players add #ok arnis.ds 1
$execute unless block $(x0) 0 $(z1) minecraft:structure_void run scoreboard players add #ok arnis.ds 1
$execute unless block $(x1) 0 $(z1) minecraft:structure_void run scoreboard players add #ok arnis.ds 1
execute unless score #ok arnis.ds matches 4 run return 0
function arnis:disaster/snow/widen
execute if score #lv_snow arnis.ds matches 1 run function arnis:disaster/snow/swap with storage arnis:ds scan
execute if score #lv_snow arnis.ds matches 0 run function arnis:disaster/snow/thaw with storage arnis:ds scan
function arnis:disaster/snow/narrow
$scoreboard players operation #c$(x)_$(z) arnis.ds.snow = #lv_snow arnis.ds
scoreboard players remove #budget arnis.ds 1
