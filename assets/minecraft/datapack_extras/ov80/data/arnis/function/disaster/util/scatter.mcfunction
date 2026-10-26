# Moves the executing marker #lo to #hi blocks in a random direction, onto the highest block there.
execute store result entity @s Rotation[0] float 1 run random value 0..359
execute store result storage arnis:ds sc.lo int 1 run scoreboard players get #lo arnis.ds
execute store result storage arnis:ds sc.hi int 1 run scoreboard players get #hi arnis.ds
function arnis:disaster/util/scatter_by with storage arnis:ds sc
