tag @s remove arnis.walk_new
execute store result score @s arnis.ws run data get storage arnis:walk spawn.s
execute store result score @s arnis.wi run data get storage arnis:walk spawn.a
scoreboard players add @s arnis.wi 1
scoreboard players set @s arnis.ww 0
scoreboard players set @s arnis.wb 0
execute store result score @s arnis.wr run data get storage arnis:walk spawn.seed
function arnis:walk/pace with storage arnis:walk spawn
function arnis:walk/enter
