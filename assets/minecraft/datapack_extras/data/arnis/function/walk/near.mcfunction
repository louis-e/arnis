execute store result score #near arnis.walk if entity @e[tag=arnis.walker,distance=..96]
execute if score #near arnis.walk >= #cap arnis.walk run return 0
execute store result score #px arnis.walk run data get entity @s Pos[0]
execute store result score #pz arnis.walk run data get entity @s Pos[2]
scoreboard players operation #px arnis.walk /= #spot_cell arnis.walk
scoreboard players operation #pz arnis.walk /= #spot_cell arnis.walk
# Well away once the streets have people, nearer while they are still empty.
scoreboard players operation #half arnis.walk = #cap arnis.walk
scoreboard players operation #half arnis.walk /= #2 arnis
scoreboard players set #sparse arnis.walk 0
execute if score #near arnis.walk < #half arnis.walk run scoreboard players set #sparse arnis.walk 1
scoreboard players set #tries arnis.walk 24
# Where this player looks, level with the street, so nobody steps out of thin air there.
execute rotated ~ 0 anchored eyes positioned ^ ^ ^40 run summon marker ~ ~ ~ {Tags:["arnis.w_view"]}
function arnis:walk/try
kill @e[type=marker,tag=arnis.w_view]
