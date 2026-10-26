scoreboard players remove @s arnis.ds.t 1
execute if score @s arnis.ds.t matches 1.. run return 0
execute positioned over ocean_floor run function arnis:disaster/meteor/remains
kill @s
