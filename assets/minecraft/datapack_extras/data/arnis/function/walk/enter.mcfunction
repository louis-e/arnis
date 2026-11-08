execute store result storage arnis:walk a.s int 1 run scoreboard players get @s arnis.ws
function arnis:walk/enter_at with storage arnis:walk a
execute if score @s arnis.wi >= @s arnis.wl run scoreboard players operation @s arnis.wi = @s arnis.wl
execute if score @s arnis.wi >= @s arnis.wl run scoreboard players remove @s arnis.wi 1
execute if score @s arnis.wi matches ..-1 run scoreboard players set @s arnis.wi 0
function arnis:walk/aim
