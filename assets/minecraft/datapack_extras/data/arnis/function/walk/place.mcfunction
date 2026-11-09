$execute positioned $(x) $(y) $(z) unless loaded ~ ~ ~ run return 0
$execute positioned $(x) $(y) $(z) unless entity @a[distance=..80] run return 0
$execute if score #sparse arnis.walk matches 0 positioned $(x) $(y) $(z) if entity @a[distance=..40] run return 0
$execute positioned $(x) $(y) $(z) if entity @a[distance=..16] run return 0
# Never in front of someone's eyes, unless far down the street.
$execute positioned $(x) $(y) $(z) if entity @e[type=marker,tag=arnis.w_view,distance=..30] if entity @a[distance=..72] run return 0
$execute positioned $(x) $(y) $(z) if entity @e[tag=arnis.walker,distance=..2] run return 0
# Who: a look, a height, a pace, mostly the usual one, and their own run of chances.
execute store result storage arnis:walk spawn.l int 1 run random value 0..17
execute store result storage arnis:walk spawn.h int 1 run random value 0..7
scoreboard players set #p arnis.walk 3
execute store result score #r arnis.walk run random value 0..15
execute if score #r arnis.walk matches 0 run scoreboard players set #p arnis.walk 0
execute if score #r arnis.walk matches 1..2 run scoreboard players set #p arnis.walk 1
execute if score #r arnis.walk matches 3..5 run scoreboard players set #p arnis.walk 2
execute if score #r arnis.walk matches 10..12 run scoreboard players set #p arnis.walk 4
execute if score #r arnis.walk matches 13..14 run scoreboard players set #p arnis.walk 5
execute if score #r arnis.walk matches 15 run scoreboard players set #p arnis.walk 6
execute store result storage arnis:walk spawn.p int 1 run scoreboard players get #p arnis.walk
execute store result storage arnis:walk spawn.seed int 1 run random value 0..2147483646
# Some walk with company: a friend, or by day a child, who sets the pace.
scoreboard players set #company arnis.walk 0
execute store result score #r arnis.walk run random value 0..99
execute if score #r arnis.walk matches ..29 run scoreboard players set #company arnis.walk 1
execute store result score #r arnis.walk run random value 0..2
execute if score #company arnis.walk matches 1 if score #r arnis.walk matches 0 unless score #daytime arnis matches 13000..22999 run scoreboard players set #company arnis.walk 2
execute if score #company arnis.walk matches 2 run data modify storage arnis:walk spawn.p set value 0
function arnis:walk/look with storage arnis:walk spawn
function arnis:walk/dress
function arnis:walk/body with storage arnis:walk spawn
execute as @e[tag=arnis.walk_new] at @s run function arnis:walk/settle
scoreboard players add #near arnis.walk 1
execute if score #company arnis.walk matches 1.. run function arnis:walk/company
