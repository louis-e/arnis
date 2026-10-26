# A burst of wind at foot height here. The charge goes off on the block below, or on a barrier
# standing in for two ticks where that is air; up any higher, the barrier shields the target.
execute if block ~ ~-1 ~ #minecraft:air positioned ~ ~-1 ~ run function arnis:disaster/util/gust_floor
summon wind_charge ~ ~0.2 ~ {Motion:[0.0d,-0.6d,0.0d],Tags:["arnis.ds.ent"]}
