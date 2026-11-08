tag @s remove arnis.walk_new
execute store result score @s arnis.ws run data get storage arnis:walk spawn.s
execute store result score @s arnis.wi run data get storage arnis:walk spawn.a
scoreboard players add @s arnis.wi 1
scoreboard players set @s arnis.ww 0
scoreboard players set @s arnis.wb 0
# Children walk at the slowest pace, everyone else at one of three.
execute store result storage arnis:walk spawn.p int 1 run random value 0..2
execute if data storage arnis:walk spawn{h:8} run data modify storage arnis:walk spawn.p set value 0
execute if data storage arnis:walk spawn{h:9} run data modify storage arnis:walk spawn.p set value 0
function arnis:walk/pace with storage arnis:walk spawn
function arnis:walk/enter
