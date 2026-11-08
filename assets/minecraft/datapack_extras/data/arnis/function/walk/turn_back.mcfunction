scoreboard players set @s arnis.wb 0
execute store result storage arnis:walk a.s int 1 run scoreboard players get @s arnis.ws
function arnis:walk/turn_back_at with storage arnis:walk a
