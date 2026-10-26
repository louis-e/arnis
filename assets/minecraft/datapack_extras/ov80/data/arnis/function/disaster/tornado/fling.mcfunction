effect clear @s minecraft:levitation
scoreboard players set @s arnis.ds.t 0
# Thrown out of the side, a little up.
execute facing entity @e[type=armor_stand,tag=arnis.ds.here,limit=1] feet rotated ~ 0 positioned ^ ^ ^1 run function arnis:disaster/util/gust
