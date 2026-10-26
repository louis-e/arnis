execute unless entity @p[gamemode=!spectator,distance=..300] run return 0
execute if entity @p[gamemode=!spectator,distance=..22] facing entity @p[gamemode=!spectator] feet rotated ~ 0 run return run rotate @s ~ 0
# Otherwise the lowest way toward the player: streets and squares before buildings.
execute store result score #gy arnis.ds run data get entity @s Pos[1]
scoreboard players set #best arnis.ds 2147483647
scoreboard players set #ba arnis.ds 0
execute facing entity @p[gamemode=!spectator] feet rotated ~ 0 run function arnis:disaster/kaiju/look
execute store result storage arnis:ds kj.a int 1 run scoreboard players get #ba arnis.ds
function arnis:disaster/kaiju/turn with storage arnis:ds kj
