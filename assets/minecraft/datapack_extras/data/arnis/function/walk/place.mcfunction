$execute positioned $(x) $(y) $(z) unless loaded ~ ~ ~ run return 0
$execute positioned $(x) $(y) $(z) unless entity @a[distance=..80] run return 0
$execute if score #sparse arnis.walk matches 0 positioned $(x) $(y) $(z) if entity @a[distance=..40] run return 0
$execute positioned $(x) $(y) $(z) if entity @a[distance=..6] run return 0
$execute positioned $(x) $(y) $(z) if entity @e[tag=arnis.walker,distance=..2] run return 0
# Who: a look, a height and a pace to suit them.
execute store result storage arnis:walk spawn.l int 1 run random value 0..17
execute store result storage arnis:walk spawn.h int 1 run random value 0..9
function arnis:walk/look with storage arnis:walk spawn
function arnis:walk/dress
function arnis:walk/body with storage arnis:walk spawn
execute as @e[tag=arnis.walk_new] at @s run function arnis:walk/settle
scoreboard players add #near arnis.walk 1
